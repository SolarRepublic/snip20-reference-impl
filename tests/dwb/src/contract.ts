import type {JsonObject, JsonValue, NaiveHexLower, NaiveHexMixed} from '@blake.regalia/belt';
import type {Snip24} from '@solar-republic/contractor';

import {readFileSync} from 'node:fs';
import {gunzip} from 'node:zlib';
import {promisify} from 'node:util';

import {base64_to_bytes, bytes, bytes_to_base64, bytes_to_hex, bytes_to_text, cast, entries, sha256, text_to_base64, transform_object} from '@blake.regalia/belt';
import {
	SecretApp,
	SecretContract,
	SecretWasm,
	TxResultTuple,
	Wallet,
	WeakSecretAccAddr,
	broadcast_result,
	create_and_sign_tx_direct,
	exec_fees,
	random_32,
} from '@solar-republic/neutrino';

import {EncodedGoogleProtobufAny, encodeGoogleProtobufAny} from '@solar-republic/cosmos-grpc/google/protobuf/any';

import {querySecretComputeCodeHashByCodeId, querySecretComputeCodes} from '@solar-republic/cosmos-grpc/secret/compute/v1beta1/query';
import {querySecretRegistrationTxKey} from '@solar-republic/cosmos-grpc/secret/registration/v1beta1/query'

import {queryCosmwasmWasmCode, queryCosmwasmWasmCodes} from '@solar-republic/cosmos-grpc/cosmwasm/wasm/v1/query';

import {SI_MESSAGE_TYPE_SECRET_COMPUTE_MSG_STORE_CODE, SI_MESSAGE_TYPE_SECRET_COMPUTE_MSG_INSTANTIATE_CONTRACT, encodeSecretComputeMsgStoreCode, encodeSecretComputeMsgInstantiateContract} from '@solar-republic/cosmos-grpc/secret/compute/v1beta1/msg';
import {CwHexLower, CwUint128, WeakUintStr} from '@solar-republic/types';
import { destructSecretRegistrationKey } from '@solar-republic/cosmos-grpc/secret/registration/v1beta1/msg';
import { safe_bytes_to_base64 } from '@solar-republic/cosmos-grpc';
import { X_GAS_PRICE, P_LOCALSECRET_LCD } from './constants';


export async function exec(k_wallet: Wallet, atu8_msg: EncodedGoogleProtobufAny, xg_gas_limit: bigint): Promise<TxResultTuple> {
	const [atu8_raw, atu8_signdoc, si_txn] = await create_and_sign_tx_direct(
		k_wallet,
		[atu8_msg],
		exec_fees(xg_gas_limit, X_GAS_PRICE, 'uscrt'),
		xg_gas_limit
	);

	return await broadcast_result(k_wallet, atu8_raw, si_txn);
}

export async function upload_code(k_wallet: Wallet, atu8_wasm: Uint8Array): Promise<WeakUintStr> {
	let atu8_bytecode = atu8_wasm;

	// gzip-encoded; decompress
	if(0x1f === atu8_wasm[0] && 0x8b === atu8_wasm[1]) {
		atu8_bytecode = await promisify(gunzip)(atu8_wasm);
	}

	// hash
	const atu8_hash = await sha256(atu8_bytecode);
	const sb16_hash = cast<CwHexLower>(bytes_to_hex(atu8_hash));

	// fetch all uploaded codes
	const [,, g_codes] = await querySecretComputeCodes(P_LOCALSECRET_LCD);

	// already uploaded
	const g_existing = g_codes?.code_infos?.find(g => g.code_hash! === sb16_hash);
	if(g_existing) {
		return g_existing.code_id as WeakUintStr;
	}

	// upload
	const [xc_code, sx_res, g_meta, atu8_data, h_events] = await exec(k_wallet, encodeGoogleProtobufAny(
		SI_MESSAGE_TYPE_SECRET_COMPUTE_MSG_STORE_CODE,
		encodeSecretComputeMsgStoreCode(
			k_wallet.addr,
			atu8_bytecode
		)
	), 30_000_000n);

	if(xc_code) throw Error(sx_res);

	return h_events!['message.code_id'][0] as WeakUintStr;
}

export async function instantiate_contract(k_wallet: Wallet, sg_code_id: WeakUintStr, h_init_msg: JsonObject): Promise<WeakSecretAccAddr> {
	const [,, g_reg] = await querySecretRegistrationTxKey(P_LOCALSECRET_LCD);
	const [atu8_cons_pk] = destructSecretRegistrationKey(g_reg!);
	const k_wasm = await SecretWasm(atu8_cons_pk!);
	const [,, g_hash] = await querySecretComputeCodeHashByCodeId(P_LOCALSECRET_LCD, sg_code_id);

	// @ts-expect-error imported types versioning
	const atu8_body = await k_wasm.encodeMsg(g_hash!.code_hash!, h_init_msg);

	const [xc_code, sx_res, g_meta, atu8_data, h_events] = await exec(k_wallet, encodeGoogleProtobufAny(
		SI_MESSAGE_TYPE_SECRET_COMPUTE_MSG_INSTANTIATE_CONTRACT,
		encodeSecretComputeMsgInstantiateContract(
			k_wallet.addr,
			null,
			sg_code_id,
			h_init_msg['name'] as string,
			atu8_body
		)
	), 10_000_000n);

	if(xc_code) {
		const s_error = g_meta?.log ?? sx_res;

		// encrypted error message
		const m_response = /(\d+):(?: \w+:)*? encrypted: (.+?): (.+?) contract/.exec(s_error);
		if(m_response) {
			// destructure match
			const [, s_index, sb64_encrypted, si_action] = m_response;

			// decrypt ciphertext
			const atu8_plaintext = await k_wasm.decrypt(base64_to_bytes(sb64_encrypted), atu8_body.slice(0, 32));

			throw Error(bytes_to_text(atu8_plaintext));
		}

		throw Error(sx_res);
	}

	return h_events!['message.contract_address'][0] as WeakSecretAccAddr;
}
