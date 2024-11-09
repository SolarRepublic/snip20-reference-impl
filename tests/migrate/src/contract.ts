import type {JsonObject} from '@blake.regalia/belt';
import type {SecretContractInterface, Snip24, FungibleTransferCall, Snip20} from '@solar-republic/contractor';
import type {EncodedGoogleProtobufAny} from '@solar-republic/cosmos-grpc/google/protobuf/any';
import type {TxResultTuple, Wallet, WeakSecretAccAddr} from '@solar-republic/neutrino';
import type {CwHexLower, CwUint64, WeakUint128Str, WeakUintStr} from '@solar-republic/types';

import {promisify} from 'node:util';
import {gunzip} from 'node:zlib';

import {base64_to_bytes, bytes_to_base64, bytes_to_hex, bytes_to_text, cast, sha256, stringify_json} from '@blake.regalia/belt';
import {queryCosmosBankBalance} from '@solar-republic/cosmos-grpc/cosmos/bank/v1beta1/query';
import {encodeGoogleProtobufAny} from '@solar-republic/cosmos-grpc/google/protobuf/any';
import {SI_MESSAGE_TYPE_SECRET_COMPUTE_MSG_STORE_CODE, SI_MESSAGE_TYPE_SECRET_COMPUTE_MSG_INSTANTIATE_CONTRACT, encodeSecretComputeMsgStoreCode, encodeSecretComputeMsgInstantiateContract} from '@solar-republic/cosmos-grpc/secret/compute/v1beta1/msg';
import {querySecretComputeCode, querySecretComputeCodeHashByCodeId, querySecretComputeCodes, querySecretComputeContractInfo} from '@solar-republic/cosmos-grpc/secret/compute/v1beta1/query';
import {destructSecretRegistrationKey} from '@solar-republic/cosmos-grpc/secret/registration/v1beta1/msg';
import {querySecretRegistrationTxKey} from '@solar-republic/cosmos-grpc/secret/registration/v1beta1/query';
import {random_bytes} from '@solar-republic/crypto';
import {SecretContract, SecretWasm, TendermintEventFilter, TendermintWs, broadcast_result, create_and_sign_tx_direct, exec_fees, random_32} from '@solar-republic/neutrino';

import {X_GAS_PRICE, P_SECRET_LCD, P_SECRET_RPC, k_wallet_a, P_MAINNET_LCD, k_wallet_admin} from './constants';

export const K_TEF_LOCAL = await TendermintEventFilter(P_SECRET_RPC);

/**
 * Executes the given message for the given sender
 * @param k_wallet 
 * @param atu8_msg 
 * @param xg_gas_limit 
 * @returns 
 */
export async function exec(k_wallet: Wallet, atu8_msg: EncodedGoogleProtobufAny, xg_gas_limit: bigint): Promise<TxResultTuple> {
	const [atu8_raw, atu8_signdoc, si_txn] = await create_and_sign_tx_direct(
		k_wallet,
		[atu8_msg],
		exec_fees(xg_gas_limit, X_GAS_PRICE, 'uscrt'),
		xg_gas_limit
	);

	return await broadcast_result(k_wallet, atu8_raw, si_txn, K_TEF_LOCAL);
}


/**
 * Uploads the given WASM bytecode to the local chain
 * @param k_wallet 
 * @param atu8_wasm 
 * @returns 
 */
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
	const [,, g_codes] = await querySecretComputeCodes(P_SECRET_LCD);

	// already uploaded
	const g_existing = g_codes?.code_infos?.find(g => g.code_hash! === sb16_hash);
	if(g_existing) {
		console.info(`Found code ID ${g_existing.code_id} already uploaded to network`);

		return g_existing.code_id as WeakUintStr;
	}

	// upload
	const [xc_code, sx_res, g_meta, atu8_data, h_events] = await exec(k_wallet, encodeGoogleProtobufAny(
		SI_MESSAGE_TYPE_SECRET_COMPUTE_MSG_STORE_CODE,
		encodeSecretComputeMsgStoreCode(
			k_wallet.addr,
			atu8_bytecode
		)
	), 30_000000n);

	if(xc_code) {
		throw Error(xc_code+': '+(g_meta?.log ?? sx_res));
	}

	return h_events!['message.code_id'][0] as WeakUintStr;
}


/**
 * Instantiates the given code into a contract
 * @param k_wallet 
 * @param sg_code_id 
 * @param h_init_msg 
 * @returns 
 */
export async function instantiate_contract(k_wallet: Wallet, sg_code_id: WeakUintStr, h_init_msg: JsonObject): Promise<WeakSecretAccAddr> {
	const [,, g_reg] = await querySecretRegistrationTxKey(P_SECRET_LCD);
	const [atu8_cons_pk] = destructSecretRegistrationKey(g_reg!);
	const k_wasm = SecretWasm(atu8_cons_pk!);
	const [,, g_hash] = await querySecretComputeCodeHashByCodeId(P_SECRET_LCD, sg_code_id);

	// @ts-expect-error imported types versioning
	const atu8_body = await k_wasm.encodeMsg(g_hash!.code_hash, h_init_msg);

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

		throw Error(s_error);
	}

	return h_events!['message.contract_address'][0] as WeakSecretAccAddr;
}


/**
 * Fetches a given contract's code from mainnet and uploads it to the local chain
 * @param sa_contract 
 * @returns 
 */
export async function replicate_code_from_mainnet(sa_contract: WeakSecretAccAddr): Promise<WeakUintStr> {
	// fetch all uploaded codes
	const [,, g_codes] = await querySecretComputeCodes(P_SECRET_LCD);

	// check for existing code
	if(g_codes?.code_infos?.find(g => ('1' as CwUint64) === g.code_id!)) {
		console.info(`Assuming code ID 1 represents original contract`);
		return '1';
	}

	console.debug(`Asking <${P_MAINNET_LCD}> for contract info on ${sa_contract}...`);

	// query mainnet for original contract's code ID
	const [,, g_info] = await querySecretComputeContractInfo(P_MAINNET_LCD, sa_contract);
	const sg_code_id = g_info?.contract_info?.code_id;

	console.debug(`Downloading WASM bytecode...`);

	// fetch the code's WASM
	const [,, g_code] = await querySecretComputeCode(P_MAINNET_LCD, sg_code_id);

	// parse base64
	const atu8_wasm = base64_to_bytes(g_code!.wasm!);

	// hash
	const atu8_hash = await sha256(atu8_wasm);
	const sb16_hash = bytes_to_hex(atu8_hash) as CwHexLower;

	console.debug(`Comparing to local chain...`);

	// already uploaded
	const g_existing = g_codes?.code_infos?.find(g => g.code_hash! === sb16_hash);
	if(g_existing) {
		console.info(`Found code ID ${g_existing.code_id} already uploaded to network`);

		return g_existing.code_id as WeakUintStr;
	}

	console.debug(`Uploading code to local chain...`);

	// upload
	return await upload_code(k_wallet_a, atu8_wasm);
}


export async function preload_original_contract(sa_contract: WeakSecretAccAddr, k_wallet: Wallet<'secret'>): Promise<SecretContract<Snip20>> {
	// ensure original code is available
	const sg_code_id = await replicate_code_from_mainnet(sa_contract);

	console.log('Code ID: '+sg_code_id);

	// instantiate
	const sa_snip = await instantiate_contract(k_wallet_a, sg_code_id, {
		name: 'original_'+bytes_to_base64(random_bytes(6)),
		symbol: 'TKN',
		decimals: 6,
		admin: k_wallet.addr,
		initial_balances: [],
		prng_seed: bytes_to_base64(random_32()),
		config: {
			public_total_supply: true,
			enable_deposit: true,
			enable_redeem: true,
			enable_mint: true,
			enable_burn: true,
		},
	});

	// instantiate
	// @ts-expect-error deep instantiation
	return await SecretContract<SecretContractInterface<{
		extends: Snip24;
		executions: {
			transfer: [FungibleTransferCall & {
				gas_target?: WeakUint128Str;
			}];
		};
	}>>(k_wallet.lcd, sa_snip);
}
