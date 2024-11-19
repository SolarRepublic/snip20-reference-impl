import type {WeakSecretAccAddr, Wallet} from '@solar-republic/neutrino';

import {bigint_greater, bigint_lesser, MutexPool} from '@blake.regalia/belt';

import {queryCosmosBankBalance} from '@solar-republic/cosmos-grpc/cosmos/bank/v1beta1/query';
import {encodeCosmosBankMsgSend, SI_MESSAGE_TYPE_COSMOS_BANK_MSG_SEND} from '@solar-republic/cosmos-grpc/cosmos/bank/v1beta1/tx';
import {encodeGoogleProtobufAny, type EncodedGoogleProtobufAny} from '@solar-republic/cosmos-grpc/google/protobuf/any';
import {broadcast_result, create_and_sign_tx_direct, exec_fees, query_secret_contract, sign_secret_query_permit} from '@solar-republic/neutrino';

import {k_wallet_a, k_wallet_admin, k_wallet_b, P_SECRET_LCD, X_GAS_PRICE} from './constants';
import {K_TEF_LOCAL, preload_original_contract} from './contract';
import {ExternallyOwnedAccount} from './eoa';
import {Evaluator, handler} from './evaluator';
import {Parser} from './parser';

const XG_UINT128_MAX = (1n << 128n) - 1n;

let xg_total_supply = 0n;

// preload sSCRT
const k_snip_original = await preload_original_contract('secret1k0jntykt7e4g3y88ltc60czgjuqdy4c9e8fzek', k_wallet_admin);


function balance(k_sender: ExternallyOwnedAccount, xg_amount: bigint) {
	k_sender.bank += xg_amount;

	// cannot be negative
	if(k_sender.bank < 0n) {
		throw Error(`Unexpected negative balance when modifying with ${xg_amount} on ${k_sender.address}`);
	}
}

function bank(k_sender: ExternallyOwnedAccount, xg_amount: bigint) {
	k_sender.bank += xg_amount;

	// cannot be negative
	if(k_sender.bank < 0n) {
		throw Error(`Unexpected negative bank when modifying with ${xg_amount} on ${k_sender.address}`);
	}
}



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

	burn: handler('amount: token', (k_sender, g_args) => {
		balance(k_sender, -g_args.amount);
		xg_total_supply -= g_args.amount;
	}),

	transferFrom: handler('amount: token, sender: account, recipient: account', (k_sender, g_args) => {
		balance(k_sender, -g_args.amount);
		balance(ExternallyOwnedAccount.at(g_args.recipient), g_args.amount);
	}),

	sendFrom: handler('amount: token, sender: account, recipient: account, msg: json', (k_sender, g_args) => {
		balance(k_sender, -g_args.amount);
		balance(ExternallyOwnedAccount.at(g_args.recipient), g_args.amount);
	}),

	burnFrom: handler('amount: token', (k_sender, g_args) => {
		balance(ExternallyOwnedAccount.at(g_args.sender), -g_args.amount);
		xg_total_supply -= g_args.amount;
	}),

	increaseAllowance: handler('amount: token, spender: account, expiration: timestamp', (k_sender, {spender:sa_spender, amount:xg_amount, expiration:n_exp}) => {
		const h_given = k_sender.allowancesGiven;
		const h_recvd = ExternallyOwnedAccount.at(sa_spender).allowancesReceived;
		const sa_sender = k_sender.address;
		const g_prev = h_given[sa_spender];

		h_given[sa_spender] = h_recvd[sa_sender] = {
			amount: bigint_lesser(XG_UINT128_MAX, (!g_prev || g_prev.expiration < Date.now()? 0n: g_prev.amount) + xg_amount),
			expiration: n_exp,
		};
	}),

	decreaseAllowance: handler('amount: token, spender: account, expiration: timestamp', (k_sender, {spender:sa_spender, amount:xg_amount, expiration:n_exp}) => {
		const h_given = ExternallyOwnedAccount.at(sa_spender).allowancesGiven;
		const h_recvd = k_sender.allowancesReceived;
		const sa_sender = k_sender.address;
		const g_prev = h_given[sa_spender];

		h_given[sa_sender] = h_recvd[sa_sender] = {
			amount: bigint_greater(0n, (!g_prev || g_prev.expiration < Date.now()? 0n: g_prev.amount) - xg_amount),
			expiration: n_exp,
		};
	}),
};


const a_aliases = [
	'Alice',
	'Bob',
	'Carol',
	'David',
];

async function bank_send(k_wallet: Wallet<'secret'>, xg_amount: bigint, a_recipients: WeakSecretAccAddr[]) {
	const a_msgs: EncodedGoogleProtobufAny[] = [];

	let sg_limit = 50_000n;

	// seed all accounts with funds for gas
	for(const sa_recipient of a_recipients) {
		const atu8_bank = encodeGoogleProtobufAny(
			SI_MESSAGE_TYPE_COSMOS_BANK_MSG_SEND,
			encodeCosmosBankMsgSend(k_wallet.addr, sa_recipient, [[`${xg_amount}`, 'uscrt']])
		);

		a_msgs.push(atu8_bank);

		sg_limit += 5_500n;
	}

	const [atu8_raw,, si_txn] = await create_and_sign_tx_direct(k_wallet, a_msgs, exec_fees(sg_limit, X_GAS_PRICE), sg_limit);

	const [xc_code, sx_res, g_meta, atu8_data, h_events] = await broadcast_result(k_wallet, atu8_raw, si_txn, K_TEF_LOCAL);

	// failed
	if(xc_code) {
		debugger;
		throw Error(g_meta?.log ?? sx_res);
	}
}

const a_genesis = ['$a', '$b', '$c', '$d'];
const a_numerics = Array(1024).fill(0).map((w, i) => `a${i}`);

const a_eoas_genesis = await Promise.all(a_genesis.map(s => ExternallyOwnedAccount.fromAlias(s)));
const a_eoas_aliased = await Promise.all(a_aliases.map(s => ExternallyOwnedAccount.fromAlias(s)));
const a_eoas_numeric = await Promise.all(a_numerics.map(s => ExternallyOwnedAccount.fromAlias(s)));

// all eoas
const a_eoas = [...a_eoas_genesis, ...a_eoas_aliased, ...a_eoas_numeric];

// each genesis account starts with 1M SCRT
for(const k_eoa of a_eoas_genesis) {
	bank(k_eoa, 1_000_000_000000n);
}

// fund all aliases
await bank_send(k_wallet_a, 5_000000n, a_eoas_aliased.map(k => k.address));

// fund first 1024 numeric accounts
await bank_send(k_wallet_b, 5_000000n, a_eoas_numeric.map(k => k.address));

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
			2 Alice
			0 Bob
			1 Carol

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

	transferFrom:
		Bob:
			8 Alice Carol

	increaseAllowance
		Bob:
			100 Alice -1m
		
	transfer:
		Bob:
			5 Alice     **fail

	burn Alice 1
	burnFrom Bob 1 Alice Carol

	---

	${s_prog_vks}
`;


const k_parser = new Parser(s_program);

const k_evaluator = new Evaluator(H_FUNCTIONS);

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


	console.log(`${s_alias || sa_owner}: ${xg_bank} // ${xg_balance}`);
})));

debugger;

const s_test = `;
	createViewingKey Alice secret

	deposit Alice 10

	multiple:
		Alice:
	
	send:

	setViewingKey

	increaseAllowance
	decreaseAllowance
	transferFrom
	batchTransferFrom
	sendFrom
	batchSendFrom
	batchTransferFrom
	burnFrom
	batchBurnFrom
	mint
	batchMint
	revokePermit
	addSupportedDenoms
	removeSupportedDenoms

	addMinters
	removeMinters
	setMinters
	changeAdmin
	setContractStatus
`;



// transfer recipient:secret1xcj amount:"10000" 
