import type {JsonValue} from '@blake.regalia/belt';
import type {SecretContractInterface, Snip24, FungibleTransferCall, Snip20, Snip20Queries, Snip24Executions, Snip26, Snip26Queries, Snip25} from '@solar-republic/contractor';
import type {EncodedGoogleProtobufAny} from '@solar-republic/cosmos-grpc/google/protobuf/any';
import type {TxResponseTuple, Wallet, SecretWasm} from '@solar-republic/neutrino';
import type {CwHexLower, CwUint64, WeakUint128Str, WeakUintStr, WeakSecretAccAddr} from '@solar-republic/types';

import {__UNDEFINED, base64_to_bytes, bytes_to_base64, bytes_to_hex, bytes_to_text, sha256} from '@blake.regalia/belt';
import {encodeGoogleProtobufAny} from '@solar-republic/cosmos-grpc/google/protobuf/any';
import {encodeSecretComputeMsgMigrateContract, SI_MESSAGE_TYPE_SECRET_COMPUTE_MSG_MIGRATE_CONTRACT} from '@solar-republic/cosmos-grpc/secret/compute/v1beta1/msg';
import {querySecretComputeCode, querySecretComputeCodes, querySecretComputeContractInfo} from '@solar-republic/cosmos-grpc/secret/compute/v1beta1/query';
import {random_bytes} from '@solar-republic/crypto';
import {SecretContract, TendermintEventFilter, broadcast_result, create_and_sign_tx_direct, random_32, secret_contract_instantiate, secret_contract_upload_code} from '@solar-republic/neutrino';

import {P_SECRET_LCD, P_SECRET_RPC, k_wallet_a, P_MAINNET_LCD} from './constants';

export type MigratedContractInterface = SecretContractInterface<{
	config: Snip26['config'] & {
		snip52_channels: {
			recvd: {
				cbor: [
					amount: bigint,
					sender: Uint8Array,
					memo_len: number,
				];
			};
			spent: {
				cbor: [
					amount: bigint,
					actions: number,
					recipient: Uint8Array,
					balance: bigint,
				];
			};
			allowance: {
				cbor: [
					amount: bigint,
					allower: Uint8Array,
					expiration: number,
				];
			};
			multirecvd: {
				schema: {
					type: 'packet[16]';
					version: 1;
					packet_size: 17;
					data: {
						label: 'packet';
						type: 'struct';
						members: [
							{
								label: 'flagsAndAmount';
								type: 'uint64';
							},
							{
								label: 'recipientId';
								type: 'bytes8';
							},
							// {
							// 	label: 'checksum';
							// 	type: 'uint8';
							// },
						];
					};
				};
			};
			multispent: {
				schema: {
					type: 'packet[16]';
					version: 1;
					packet_size: 24;
					data: {
						label: 'packet';
						type: 'struct';
						members: [
							{
								label: 'flagsAndAmount';
								type: 'uint64';
							},
							{
								label: 'recipientId';
								type: 'bytes8';
							},
							// {
							// 	label: 'checksum';
							// 	type: 'uint8';
							// },
							{
								label: 'balance';
								type: 'uint64';
							},
						];
					};
				};
			};
		};
	};
	executions: Snip24Executions & {};
	queries: Snip26Queries & {
		legacy_transfer_history: Snip20Queries['transfer_history'];
	};
}>;

export const K_TEF_LOCAL = await TendermintEventFilter(P_SECRET_RPC, __UNDEFINED, () => {
	console.error(`WebSocket error`);
});

console.log(`🔌 Connected to ${P_SECRET_RPC}/websocket`);

/**
 * Executes the given message for the given sender
 * @param k_wallet 
 * @param atu8_msg 
 * @param xg_gas_limit 
 * @returns 
 */
export async function exec(k_wallet: Wallet, atu8_msg: EncodedGoogleProtobufAny, xg_gas_limit: bigint): Promise<TxResponseTuple> {
	const [atu8_raw, si_txn] = await create_and_sign_tx_direct(
		k_wallet,
		[atu8_msg],
		xg_gas_limit
	);

	return await broadcast_result(k_wallet, atu8_raw, si_txn, K_TEF_LOCAL);
}


/**
 * Fetches a given contract's code from mainnet and uploads it to the local chain
 * @param sa_contract 
 * @returns 
 */
export async function replicate_code_from_mainnet(sa_contract: WeakSecretAccAddr): ReturnType<typeof secret_contract_upload_code> {
	// fetch all uploaded codes
	const [g_codes] = await querySecretComputeCodes(P_SECRET_LCD);

	console.debug(`Asking <${P_MAINNET_LCD}> for contract info on ${sa_contract}...`);

	// query mainnet for original contract's code ID
	const [g_info] = await querySecretComputeContractInfo(P_MAINNET_LCD, sa_contract);
	const sg_code_id = g_info?.contract_info?.code_id;

	console.debug(`Downloading WASM bytecode...`);

	// fetch the code's WASM
	const [g_code] = await querySecretComputeCode(P_MAINNET_LCD, sg_code_id);

	// parse base64
	const atu8_wasm = base64_to_bytes(g_code!.wasm!);

	// hash
	const atu8_hash = await sha256(atu8_wasm);
	const sb16_hash = bytes_to_hex(atu8_hash) as CwHexLower;

	console.debug(`Comparing to local chain...`);

	// already uploaded
	const g_existing = g_codes?.code_infos?.find(g => g.code_hash === sb16_hash);
	if(g_existing) {
		console.info(`Found code ID ${g_existing.code_id} already uploaded to network`);

		return [g_existing.code_id!, g_existing.code_hash!];
	}

	console.debug(`Uploading code to local chain...`);

	// upload
	return await secret_contract_upload_code(k_wallet_a, atu8_wasm, 30_000000n);
}


export async function preload_original_contract(
	sa_contract: WeakSecretAccAddr,
	k_wallet: Wallet<'secret'>
): Promise<SecretContract<Snip25>> {
	// ensure original code is available
	const [sg_code_id] = await replicate_code_from_mainnet(sa_contract);

	console.log('Code ID: '+sg_code_id);

	// instantiate
	const [[sa_snip, a2_ans]=[], [xc_code, s_err]=[]] = await secret_contract_instantiate(k_wallet, sg_code_id!, {
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
		supported_denoms: ['uscrt'],
	}, 5_000_000n, [k_wallet.addr]);

	// error
	if(xc_code) {
		throw Error(`While attempting to instantiate original contract: ${s_err}`);
	}

	// instantiate
	// @ts-expect-error deep instantiation
	return await SecretContract<SecretContractInterface<{
		extends: Snip24;
		executions: {
			transfer: [FungibleTransferCall & {
				gas_target?: WeakUint128Str;
			}];
		};
	}>>(k_wallet.lcd, sa_snip!);
}

export async function migrate_contract(
	sa_contract: WeakSecretAccAddr,
	k_wallet: Wallet<'secret'>,
	sg_code_id: WeakUintStr,
	k_wasm: SecretWasm,
	sb16_codehash: CwHexLower,
	g_msg: JsonValue={}
) {
	// encrypt migrate message
	const atu8_body = await k_wasm.encodeMsg(sb16_codehash, g_msg);

	// execute migrate message
	const [xc_code, sx_res,, g_meta, atu8_data, h_events] = await exec(k_wallet, encodeGoogleProtobufAny(
		SI_MESSAGE_TYPE_SECRET_COMPUTE_MSG_MIGRATE_CONTRACT,
		encodeSecretComputeMsgMigrateContract(
			k_wallet.addr,
			sa_contract,
			sg_code_id,
			atu8_body
		)
	), 600_000n);

	if(xc_code) {
		const s_error = g_meta?.log ?? sx_res;

		// encrypted error message
		const m_response = /(\d+):(?: \w+:)*? encrypted: (.+?): (.+?) contract/.exec(s_error);
		if(m_response) {
			// destructure match
			const [, s_index, sb64_encrypted, si_action] = m_response;

			// decrypt ciphertext
			const atu8_plaintext = await k_wasm.decrypt(base64_to_bytes(sb64_encrypted), atu8_body.slice(0, 32));

			throw Error(`During ${si_action} action at message #${s_index}: ${bytes_to_text(atu8_plaintext)}`);
		}

		throw Error(s_error);
	}
}
