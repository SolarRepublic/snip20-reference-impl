import {readFileSync} from 'node:fs';

import {bigint_greater, bigint_lesser, MutexPool} from '@blake.regalia/belt';

import {queryCosmosBankBalance} from '@solar-republic/cosmos-grpc/cosmos/bank/v1beta1/query';
import {querySecretComputeCodeHashByCodeId} from '@solar-republic/cosmos-grpc/secret/compute/v1beta1/query';
import {destructSecretRegistrationKey} from '@solar-republic/cosmos-grpc/secret/registration/v1beta1/msg';
import {querySecretRegistrationTxKey} from '@solar-republic/cosmos-grpc/secret/registration/v1beta1/query';
import {query_secret_contract, SecretWasm, sign_secret_query_permit} from '@solar-republic/neutrino';

import {k_wallet_a, k_wallet_admin, P_SECRET_LCD, SR_LOCAL_WASM} from './constants';
import {migrate_contract, preload_original_contract, upload_code} from './contract';
import {bank, balance, bank_send} from './cosmos';
import {ExternallyOwnedAccount} from './eoa';
import {Evaluator, handler} from './evaluator';
import {Parser} from './parser';

const XG_UINT128_MAX = (1n << 128n) - 1n;

let xg_total_supply = 0n;

const SA_MAINNET_SSCRT = 'secret1k0jntykt7e4g3y88ltc60czgjuqdy4c9e8fzek';

// preload sSCRT
const k_snip_original = await preload_original_contract(SA_MAINNET_SSCRT, k_wallet_admin);

/*
available sSCRT methods:
	`redeem`,
	`deposit`,
	`transfer`,
	`send`,
	`register_receive`,
	`create_viewing_key`,
	`set_viewing_key`,
	`increase_allowance`,
	`decrease_allowance`,
	`transfer_from`,
	`send_from`,
	`change_admin`,
	`set_contract_status`
*/

/**
 * suite handlers for tracking and double checking expected states in contract
 */
const H_FUNCTIONS = {
	createViewingKey: handler('entropy: string => key: string', (k_sender, g_args, g_answer) => {
		k_sender.viewingKey = g_answer.key;
	}),

	setViewingKey: handler('key: string', (k_sender, g_args) => {
		k_sender.viewingKey = g_args.key;
	}),

	deposit: handler('amount: token', (k_sender, g_args) => {
		bank(k_sender, -g_args.amount);
		balance(k_sender, g_args.amount);
	}, {
		before: (k_eoa, g_args) => ({
			funds: g_args.amount,
		}),
	}),

	redeem: handler('amount: token', (k_sender, g_args) => {
		balance(k_sender, -g_args.amount);
		bank(k_sender, g_args.amount);
	}),

	transfer: handler('amount: token, recipient: account', (k_sender, g_args) => {
		balance(k_sender, -g_args.amount);
		balance(ExternallyOwnedAccount.at(g_args.recipient), g_args.amount);
	}),

	send: handler('amount: token, recipient: account, msg: json', (k_sender, g_args) => {
		balance(k_sender, -g_args.amount);
		balance(ExternallyOwnedAccount.at(g_args.recipient), g_args.amount);
	}),
	transferFrom: handler('amount: token, owner: account, recipient: account', (k_sender, g_args) => {
		balance(ExternallyOwnedAccount.at(g_args.owner), -g_args.amount);
		balance(ExternallyOwnedAccount.at(g_args.recipient), g_args.amount);
	}),

	sendFrom: handler('amount: token, owner: account, recipient: account, msg: json', (k_sender, g_args) => {
		balance(ExternallyOwnedAccount.at(g_args.owner), -g_args.amount);
		balance(ExternallyOwnedAccount.at(g_args.recipient), g_args.amount);
	}),

	increaseAllowance: handler('amount: token, spender: account, expiration: timestamp', (k_sender, {spender:sa_spender, amount:xg_amount, expiration:n_exp}) => {
		const h_given = k_sender.allowancesGiven;
		const h_recvd = ExternallyOwnedAccount.at(sa_spender).allowancesReceived;
		const sa_sender = k_sender.address;
		const g_prev = h_given[sa_spender];

		h_given[sa_spender] = h_recvd[sa_sender] = {
			amount: bigint_lesser(XG_UINT128_MAX, (!g_prev?.expiration || g_prev.expiration < Date.now()? 0n: g_prev.amount) + xg_amount),
			expiration: n_exp,
		};
	}),

	decreaseAllowance: handler('amount: token, spender: account, expiration: timestamp', (k_sender, {spender:sa_spender, amount:xg_amount, expiration:n_exp}) => {
		const h_given = ExternallyOwnedAccount.at(sa_spender).allowancesGiven;
		const h_recvd = k_sender.allowancesReceived;
		const sa_sender = k_sender.address;
		const g_prev = h_given[sa_spender];

		h_given[sa_sender] = h_recvd[sa_sender] = {
			amount: bigint_greater(0n, (!g_prev?.expiration || g_prev.expiration < Date.now()? 0n: g_prev.amount) - xg_amount),
			expiration: n_exp,
		};
	}),

	burn: handler('amount: token', (k_sender, g_args) => {
		balance(k_sender, -g_args.amount);
		xg_total_supply -= g_args.amount;
	}),

	burnFrom: handler('amount: token, owner: account', (k_sender, g_args) => {
		balance(ExternallyOwnedAccount.at(g_args.owner), -g_args.amount);
		xg_total_supply -= g_args.amount;
	}),
};


// genesis accounts
const a_genesis = ['$a', '$b', '$c', '$d'];

// alias accounts
const a_aliases = [
	'Alice',
	'Bob',
	'Carol',
	'David',
];

// numeric accounts
const a_numerics = Array(1024).fill(0).map((w, i) => `a${i}`);

// instantiate EOAs for all accounts
const a_eoas_genesis = await Promise.all(a_genesis.map(s => ExternallyOwnedAccount.fromAlias(s)));
const a_eoas_aliased = await Promise.all(a_aliases.map(s => ExternallyOwnedAccount.fromAlias(s)));
const a_eoas_numeric = await Promise.all(a_numerics.map(s => ExternallyOwnedAccount.fromAlias(s)));

// concat all eoas
const a_eoas = [...a_eoas_genesis, ...a_eoas_aliased, ...a_eoas_numeric];

// determine how much uscrt each genesis account actually has
for(const k_eoa of a_eoas_genesis) {
	const [,, g_bank] = await queryCosmosBankBalance(P_SECRET_LCD, k_eoa.address, 'uscrt');
	bank(k_eoa, BigInt(g_bank?.balance?.amount ?? '0'));
}

// fund all aliases
await bank_send(a_eoas_genesis[0], 5_000000n, a_eoas_aliased);

// fund first 1024 numeric accounts
await bank_send(a_eoas_genesis[1], 5_000000n, a_eoas_numeric);

// sign query permit for all accounts
await Promise.all([
	...a_eoas_aliased,
	...a_eoas_numeric,
].map(async(k_eoa) => {
	k_eoa.queryPermit = await sign_secret_query_permit(k_eoa.wallet, 'balance', [k_snip_original.addr], ['owner', 'balance']);
}));

// set or create viewing keys for other accounts
const s_prog_vks = [
	...a_aliases,
	...a_numerics,
].map((si, i) => `${i % 2? 'create': 'set'}ViewingKey ${si} ${si}`).join('\n\t');


/* syntax:
[action] [sender] [...args]
*/
const s_program = `
	createViewingKey $a $a
	deposit $a 100_000_000_000
	transfer $a 10_000_000 Alice

	deposit $b 100_000_000_000
	createViewingKey $b $b
	transfer $b 10_000_000 Bob

	setViewingKey $c plain
	deposit $c 100_000_000_000
	transfer $c 10_000_000 Carol

	deposit $d 100_000_000_000
	transfer $d 10_000_000 David
	setViewingKey $d plain

	---

	transfer $a 100_000_000 Alice
	transfer $b 100_000_000 Bob
	transfer $c 100_000_000 Carol
	transfer $d 100_000_000 David

	---

	transfer:
		Alice:
			90 Bob
			5  Carol
			1  David

		Bob:
			5 Carol
			2 David
			1 Alice
		
		Carol:
			1 Alice
			1 Bob
			1 David
			0 David
			1 Carol
			0 Carol
	
	send:
		David:
			2 David
			0 David
			2 Alice {}
			1 Alice {"confirm":{}}
			0 Bob {}
			1 Carol
			1 Carol {}

	---

	increaseAllowance:
		Alice:
			10 Bob
			2 Carol
			1 David
	
	decreaseAllowance:
		Alice:
			2 Bob
			10 Carol
			1 David
	
	increaseAllowance:
		Alice:
			0 Bob
			2 Carol
			1 David

	---

	transferFrom:
		Bob:
			8 Alice Carol
			0 Carol David
			1000 Alice David    **fail insufficient allowance

	increaseAllowance:
		Bob:
			100 Alice -1m

	---
	
	transferFrom:
		Alice:
			5 Bob Carol         **fail insufficient allowance

	---

	redeem:
		Alice 20
		Bob 0                  **fail invalid coins
		Carol 1
		David 900_000_000_000  **fail insufficient funds

	${s_prog_vks}
`;


// parse program
const k_parser = new Parser(s_program);

// prep evaluator
const k_evaluator = new Evaluator(H_FUNCTIONS);

// evaluate program
await k_evaluator.evaluate(k_parser, k_snip_original);

// concurrency
const kl_queries = MutexPool(8);

// each eoa in batches
await Promise.all(a_eoas.map(k_eoa => kl_queries.use(async() => {
	// destructure eoa
	const {
		bank: xg_bank,
		balance: xg_balance,
		address: sa_owner,
		alias: s_alias,
	} = k_eoa;

	// resolve
	const [[,, g_bank], a_results] = await Promise.all([
		// query bank module
		queryCosmosBankBalance(P_SECRET_LCD, sa_owner, 'uscrt'),

		// query contract balance
		query_secret_contract(k_snip_original, 'balance', {
			address: sa_owner,
		}, k_eoa.viewingKey),
	]);

	// assert bank balances match
	if(`${xg_bank}` !== g_bank?.balance?.amount) {
		throw Error(`Bank discrepancy for ${s_alias || sa_owner}; suite accounts for ${xg_bank} but bank module reports ${g_bank?.balance?.amount}`);
	}

	// detuple results
	const [g_balance] = a_results;

	// assert that the SNIP balances are identical
	if(`${xg_balance}` !== g_balance?.amount) {
		debugger;
		throw Error(`Balance discrepancy for ${s_alias || sa_owner}; suite accounts for ${xg_balance} but contract reports ${g_balance?.amount}`);
	}

	// TODO: query allowances
})));

//
console.log(`✅ Verified ${a_eoas.length} bank balances and SNIP balances`);

// read WASM file
const atu8_wasm = readFileSync(SR_LOCAL_WASM);

// upload code to chain
console.debug(`Uploading code...`);
const [sg_code_id, sb16_codehash] = await upload_code(k_wallet_a, atu8_wasm);

{
	console.debug('Encoding migration message...');
	const [,, g_reg] = await querySecretRegistrationTxKey(P_SECRET_LCD);
	const [atu8_cons_pk] = destructSecretRegistrationKey(g_reg!);
	const k_wasm = SecretWasm(atu8_cons_pk!);

	// encrypt migrate message
	const atu8_msg = await k_wasm.encodeMsg(sb16_codehash, {});

	// run migration
	console.debug(`Running migration...`);
	const a_data = await migrate_contract(k_snip_original.addr, k_wallet_a, sg_code_id, atu8_msg);
	debugger;
	console.log(a_data);
}

