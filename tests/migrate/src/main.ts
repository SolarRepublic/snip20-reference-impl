
import type {SecretContractInterface, FungibleTransferCall, Snip24} from '@solar-republic/contractor';

import type {WeakUint128Str} from '@solar-republic/types';

import {readFileSync} from 'node:fs';

import {bytes, bytes_to_base64, entries} from '@blake.regalia/belt';
import {SecretApp, SecretContract, random_32} from '@solar-republic/neutrino';

import {P_SECRET_LCD, X_GAS_PRICE, k_wallet_a, k_wallet_admin, k_wallet_b, k_wallet_c, k_wallet_d} from './constants';
import {upload_code, instantiate_contract} from './contract';

// make unique contract label
const S_CONTRACT_LABEL = 'snip2x-test_'+bytes_to_base64(crypto.getRandomValues(bytes(6)));

// read WASM file
const atu8_wasm = readFileSync('../../contract.wasm');

// upload code to chain
console.debug(`Uploading code...`);
const sg_code_id = await upload_code(k_wallet_a, atu8_wasm);

// instantiate contract
console.debug(`Instantiating contract...`);
const sa_snip = await instantiate_contract(k_wallet_a, sg_code_id, {
	name: S_CONTRACT_LABEL,
	symbol: 'TKN',
	decimals: 6,
	admin: k_wallet_admin.addr,
	initial_balances: [],
	// initial_balances: entries({
	// 	[k_wallet_a.addr]: 10_000_000000n,
	// }).map(([sa_account, xg_balance]) => ({
	// 	address: sa_account,
	// 	amount: `${xg_balance}`,
	// })),
	prng_seed: bytes_to_base64(random_32()),
	config: {
		public_total_supply: true,
		enable_deposit: true,
		enable_redeem: true,
		enable_mint: true,
		enable_burn: true,
	},
});

console.debug(`Running tests against ${sa_snip}...`);

// @ts-expect-error deep instantiation
const k_contract = await SecretContract<SecretContractInterface<{
	extends: Snip24;
	executions: {
		transfer: [FungibleTransferCall & {
			gas_target?: WeakUint128Str;
		}];
	};
}>>(P_SECRET_LCD, sa_snip);

const k_app_a = SecretApp(k_wallet_a, k_contract, X_GAS_PRICE);
const k_app_b = SecretApp(k_wallet_b, k_contract, X_GAS_PRICE);
const k_app_c = SecretApp(k_wallet_c, k_contract, X_GAS_PRICE);
const k_app_d = SecretApp(k_wallet_d, k_contract, X_GAS_PRICE);

const H_APPS = {
	a: k_app_a,
	b: k_app_b,
	c: k_app_c,
	d: k_app_d,
};



// {
// 	const k_app_sim = SecretApp(k_wallet, k_contract, X_GAS_PRICE);

// 	// label
// 	console.log(`Alice --> ${si_receiver}`);

// 	// transfer some gas to sim account
// 	const [atu8_raw,, si_txn] = await create_and_sign_tx_direct(k_wallet_b, [
// 		encodeGoogleProtobufAny(
// 			SI_MESSAGE_TYPE_COSMOS_BANK_MSG_SEND,
// 			encodeCosmosBankMsgSend(k_wallet_b.addr, k_wallet.addr, [[`${1_000000n}`, 'uscrt']])
// 		),
// 	], [[`${5000n}`, 'uscrt']], 50_000n);

// 	// submit all in parallel
// 	const [
// 		// @ts-expect-error totally stupid
// 		g_result_transfer,
// 		[xc_send_gas, s_err_send_gas],
// 		a_res_increase,
// 	] = await Promise.all([
// 		// #ts-expect-error secret app
// 		transfer(k_dwbv, i_sim % 2? 1_000000n: 2_000000n, k_app_a, k_app_sim, k_checker),
// 		broadcast_result(k_wallet, atu8_raw, si_txn),
// 		f_grant?.(),
// 	]);

// 	// send gas error
// 	if(xc_send_gas) {
// 		throw Error(`Failed to transfer gas: ${s_err_send_gas}`);
// 	}

// 	// increase allowance error
// 	if(f_grant && a_res_increase?.[1]) {
// 		throw Error(`Failed to increase allowance: ${a_res_increase[2]}`);
// 	}

// 	// approve Alice as spender for future txs
// 	f_grant = () => k_app_sim.exec('increase_allowance', {
// 		spender: k_wallet_a.addr,
// 		amount: `${1_000000n}` as CwUint128,
// 	}, 60_000n);

// 	if(!k_checker) {
// 		k_checker = new GasChecker((g_result_transfer as TransferResult).tracking, (g_result_transfer as TransferResult).gasUsed);
// 	}

// 	xg_max_gas_used_transfer = bigint_greater(xg_max_gas_used_transfer, g_result_transfer.gasUsed);

// }
