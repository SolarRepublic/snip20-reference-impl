import type {Snip24} from '@solar-republic/contractor';
import type {CwUint128} from '@solar-republic/types';


import {readFileSync} from 'node:fs';

import {F_IDENTITY, base64_to_bytes, bytes, bytes_to_base64, bytes_to_hex, entries, sha256} from '@blake.regalia/belt';

import {SecretApp, SecretContract, Wallet, WeakSecretAccAddr, expect_tx, random_32, sign_secret_query_permit} from '@solar-republic/neutrino';
import {querySecretComputeCodes} from '@solar-republic/cosmos-grpc/secret/compute/v1beta1/query';

import {P_LOCALSECRET_LCD, P_LOCALSECRET_RPC, X_GAS_PRICE, k_wallet_a, k_wallet_b, k_wallet_c, k_wallet_d} from './constants';
import {upload_code, instantiate_contract} from './contract';
import { DwbValidator, parse_dwb_dump } from './dwb';
import { fail } from './helper';
import { transfer } from './snip';

const S_CONTRACT_LABEL = 'snip2x-test_'+bytes_to_base64(crypto.getRandomValues(bytes(6)));


const atu8_wasm = readFileSync('../../contract.wasm');

console.debug(`Uploading code...`);
const sg_code_id = await upload_code(k_wallet_a, atu8_wasm);

console.debug(`Instantiating contract...`);
// const sa_snip = 'secret1mfk7n6mc2cg6lznujmeckdh4x0a5ezf6hx6y8q' ||
const sa_snip = await instantiate_contract(k_wallet_a, sg_code_id, {
	name: S_CONTRACT_LABEL,
	symbol: 'TKN',
	decimals: 6,
	admin: k_wallet_a.addr,
	initial_balances: entries({
		[k_wallet_a.addr]: 100_000000n,
	}).map(([sa_account, xg_balance]) => ({
		address: sa_account,
		amount: `${xg_balance}`,
	})),
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
const k_contract = await SecretContract<Snip24>(P_LOCALSECRET_LCD, sa_snip);

const k_app_a = await SecretApp(k_wallet_a, k_contract, X_GAS_PRICE);
const k_app_b = await SecretApp(k_wallet_b, k_contract, X_GAS_PRICE);
const k_app_c = await SecretApp(k_wallet_c, k_contract, X_GAS_PRICE);
const k_app_d = await SecretApp(k_wallet_d, k_contract, X_GAS_PRICE);

const k_dwbv = new DwbValidator(k_app_a);

console.log('# Initialized');
await k_dwbv.sync();
await k_dwbv.print();
console.log('\n');

{
	// 1 TKN Alice => Bob
	await transfer(k_dwbv, 1_000000n, k_app_a, k_app_b);

	// 2 TKN Alice => Carol
	await transfer(k_dwbv, 2_000000n, k_app_a, k_app_c);

	// 5 TKN Alice => David
	await transfer(k_dwbv, 5_000000n, k_app_a, k_app_d);

	// 1 TKN Bob => Carol (Bob's entire balance; settles Bob for 1st time)
	await transfer(k_dwbv, 1_000000n, k_app_b, k_app_c);

	// 1 TKN Carol => David (should accumulate; settles Carol for 1st time)
	await transfer(k_dwbv, 1_000000n, k_app_c, k_app_d);

	// 1 TKN David => Alice (re-adds Alice to buffer; settles David for 1st time)
	await transfer(k_dwbv, 1_000000n, k_app_d, k_app_a);

	// all operations should be same gas from now on
	/*
	Alice: 93
	Bob: 0
	Carol: 1
	David: 4
	*/

	console.log('--- should all be same gas ---');

	// 1 TKN David => Bob
	await transfer(k_dwbv, 1_000000n, k_app_d, k_app_b);

	// 1 TKN David => Bob (exact same transfer repeated)
	await transfer(k_dwbv, 1_000000n, k_app_d, k_app_b);

	// 1 TKN Alice => Bob
	await transfer(k_dwbv, 1_000000n, k_app_a, k_app_b);

	// 1 TKN Bob => Carol
	await transfer(k_dwbv, 1_000000n, k_app_b, k_app_c);

	// 1 TKN Alice => Carol
	await transfer(k_dwbv, 1_000000n, k_app_a, k_app_c);

	// 1 TKN Carol => Bob (yet again)
	await transfer(k_dwbv, 1_000000n, k_app_c, k_app_b);
}
