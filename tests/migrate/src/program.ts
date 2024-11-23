import type {Snip20TransferEvent, Snip250TxEvent} from './types';
import type {Snip20, Snip20Queries, Snip26, Snip24Executions, Snip24Queries, Snip26Queries, SecretAccAddr} from '@solar-republic/contractor';


import {readFileSync} from 'node:fs';

import {__UNDEFINED, bigint_greater, bigint_lesser, canonicalize_json, keys, MutexPool, stringify_json} from '@blake.regalia/belt';

import {queryCosmosBankBalance} from '@solar-republic/cosmos-grpc/cosmos/bank/v1beta1/query';
import {destructSecretRegistrationKey} from '@solar-republic/cosmos-grpc/secret/registration/v1beta1/msg';
import {querySecretRegistrationTxKey} from '@solar-republic/cosmos-grpc/secret/registration/v1beta1/query';
import {SecretContract, XC_CONTRACT_CACHE_BYPASS, query_secret_contract, SecretWasm, sign_secret_query_permit} from '@solar-republic/neutrino';

import {k_wallet_a, P_SECRET_LCD, SR_LOCAL_WASM} from './constants';
import {migrate_contract, preload_original_contract, upload_code} from './contract';
import {bank, balance, bank_send} from './cosmos';
import {ExternallyOwnedAccount} from './eoa';
import {Evaluator, handler} from './evaluator';
import {Parser} from './parser';

const XG_UINT128_MAX = (1n << 128n) - 1n;

let xg_total_supply = 0n;

const SA_MAINNET_SSCRT = 'secret1k0jntykt7e4g3y88ltc60czgjuqdy4c9e8fzek';

// preload sSCRT
const k_snip_original = await preload_original_contract(SA_MAINNET_SSCRT, k_wallet_a);
let k_snip_migrated: SecretContract<{
	config: Snip26['config'];
	executions: Snip24Executions & {};
	queries: Snip26Queries & {
		legacy_transfer_history: Snip20Queries['transfer_history'];
	};
}>;

// native denom
let s_native_denom = 'uscrt';


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

function transfer_from(
	k_sender: ExternallyOwnedAccount,
	k_recipient: ExternallyOwnedAccount,
	xg_amount: bigint,
	k_from: ExternallyOwnedAccount=k_sender
) {
	// migrated
	if(k_snip_migrated) {
		// init migration
		k_from.migrate();
		k_recipient.migrate();

		// create event
		const g_event: Snip250TxEvent = {
			action: {
				transfer: {
					from: k_from.address,
					sender: k_sender.address,
					recipient: k_recipient.address,
				},
			},
			coins: {
				denom: 'TKN',
				amount: `${xg_amount}` as const,
			},
		};

		// add to histories
		k_from.txs.push(g_event);
		k_recipient.txs.push(g_event);

		// add to sender history as well
		if(k_from !== k_sender) {
			k_sender.migrate();
			k_sender.txs.push(g_event);
		}
	}
	// legacy
	else {
		// create legacy event
		const g_event: Snip20TransferEvent = {
			from: k_from.address,
			sender: k_sender.address,
			receiver: k_recipient.address,
			coins: {
				denom: 'TKN',
				amount: `${xg_amount}` as const,
			},
		};

		// add to histories
		k_from.transfers.push(g_event);
		k_recipient.transfers.push(g_event);

		// add to sender history as well
		if(k_from !== k_sender) k_sender.transfers.push(g_event);
	}

	// update balances
	balance(k_from, -xg_amount);
	balance(k_recipient, xg_amount);
}

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
		// post-migration
		if(k_snip_migrated) {
			k_sender.migrate();

			// add tx to history
			k_sender.txs.push({
				action: {
					deposit: {},
				},
				coins: {
					denom: s_native_denom,
					amount: `${g_args.amount}`,
				},
			});
		}

		// update balances
		bank(k_sender, -g_args.amount);
		balance(k_sender, g_args.amount);
	}, {
		before: (k_eoa, g_args) => ({
			funds: g_args.amount,
		}),
	}),

	redeem: handler('amount: token', (k_sender, g_args) => {
		// post-migration
		if(k_snip_migrated) {
			k_sender.migrate();

			// add tx to history
			k_sender.txs.push({
				action: {
					redeem: {},
				},
				coins: {
					denom: 'TKN',
					amount: `${g_args.amount}`,
				},
			});
		}

		// update balances
		balance(k_sender, -g_args.amount);
		bank(k_sender, g_args.amount);
	}),

	transfer: handler('amount: token, recipient: account', (k_sender, g_args) => {
		transfer_from(k_sender, ExternallyOwnedAccount.at(g_args.recipient), g_args.amount);
	}),

	send: handler('amount: token, recipient: account, msg: json', (k_sender, g_args) => {
		transfer_from(k_sender, ExternallyOwnedAccount.at(g_args.recipient), g_args.amount);
	}),

	transferFrom: handler('amount: token, owner: account, recipient: account', (k_sender, g_args) => {
		transfer_from(k_sender, ExternallyOwnedAccount.at(g_args.recipient), g_args.amount, ExternallyOwnedAccount.at(g_args.owner));
	}),

	sendFrom: handler('amount: token, owner: account, recipient: account, msg: json', (k_sender, g_args) => {
		transfer_from(k_sender, ExternallyOwnedAccount.at(g_args.recipient), g_args.amount, ExternallyOwnedAccount.at(g_args.owner));
	}),

	increaseAllowance: handler('amount: token, spender: account, expiration: timestamp', (k_sender, {spender:sa_spender, amount:xg_amount, expiration:n_exp}) => {
		const h_given = k_sender.allowancesGiven;
		const h_recvd = ExternallyOwnedAccount.at(sa_spender).allowancesReceived;
		const sa_sender = k_sender.address;
		const g_prev = h_given[sa_spender];

		if(k_snip_migrated) {
			h_given[sa_spender] = h_recvd[sa_sender] = {
				amount: bigint_lesser(XG_UINT128_MAX, (!g_prev?.expiration || g_prev.expiration < Date.now()? 0n: g_prev.amount) + xg_amount),
				expiration: n_exp,
			};
		}
	}),

	decreaseAllowance: handler('amount: token, spender: account, expiration: timestamp', (k_sender, {spender:sa_spender, amount:xg_amount, expiration:n_exp}) => {
		// const h_given = ExternallyOwnedAccount.at(sa_spender).allowancesGiven;
		// const h_recvd = k_sender.allowancesReceived;

		const h_given = k_sender.allowancesGiven;
		const h_recvd = ExternallyOwnedAccount.at(sa_spender).allowancesReceived;
		const sa_sender = k_sender.address;
		const g_prev = h_given[sa_spender];

		if(k_snip_migrated) {
			h_given[sa_sender] = h_recvd[sa_sender] = {
				amount: bigint_greater(0n, (!g_prev?.expiration || g_prev.expiration < Date.now()? 0n: g_prev.amount) - xg_amount),
				expiration: n_exp,
			};
		}
	}),

	burn: handler('amount: token', (k_sender, g_args) => {
		// post-migration
		if(k_snip_migrated) {
			k_sender.migrate();

			// add tx to history
			k_sender.txs.push({
				action: {
					burn: {
						burner: k_sender.address,
						owner: k_sender.address,
					},
				},
				coins: {
					denom: 'TKN',
					amount: `${g_args.amount}`,
				},
			});
		}

		// update balances
		balance(k_sender, -g_args.amount);
		xg_total_supply -= g_args.amount;
	}),

	burnFrom: handler('amount: token, owner: account', (k_sender, g_args) => {
		// post-migration
		if(k_snip_migrated) {
			k_sender.migrate();

			// add tx to history
			k_sender.txs.push({
				action: {
					burn: {
						burner: k_sender.address,
						owner: g_args.owner,
					},
				},
				coins: {
					denom: 'TKN',
					amount: `${g_args.amount}`,
				},
			});
		}

		// update balances
		balance(ExternallyOwnedAccount.at(g_args.owner), -g_args.amount);
		xg_total_supply -= g_args.amount;
	}),

	migrateLegacyAccount: handler('padding: string', (k_sender, g_args) => {
		k_sender.migrate();
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
const nl_nums = 128;
const a_numerics = Array(nl_nums).fill(0).map((w, i) => `a${i}`);

// instantiate EOAs for all accounts
const a_eoas_genesis = await Promise.all(a_genesis.map(s => ExternallyOwnedAccount.fromAlias(s)));
const a_eoas_aliased = await Promise.all(a_aliases.map(s => ExternallyOwnedAccount.fromAlias(s)));
const a_eoas_numeric = await Promise.all(a_numerics.map(s => ExternallyOwnedAccount.fromAlias(s)));

// concat all eoas
const a_eoas = [...a_eoas_genesis, ...a_eoas_aliased, ...a_eoas_numeric];

// determine how much uscrt each genesis account actually has
for(const k_eoa of a_eoas) {
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


// transfers
const s_prog_xfers = `
	${a_numerics.map((si, i) => `transfer $d ${nl_nums + 2} ${si}`).join('\n\t')}
	---
	${a_numerics.map((si, i) => `transfer ${si} ${i+1} a${(i+1) % nl_nums}`).join('\n\t')}
	---
	${a_numerics.map((si, i) => `redeem ${si} ${i+1}`).join('\n\t')}
`;

// set or create viewing keys for other accounts
const s_prog_vks = [
	...a_aliases,
	...a_numerics,
].map((si, i) => `${i % 2? 'create': 'set'}ViewingKey ${si} ${si}`).join('\n\t');

const s_prog_genesis = `
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
`;


/* syntax:
[action] [sender] [...args]
*/
const program = (b_premigrate: boolean) => `
	${b_premigrate? s_prog_genesis: ''}

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
			1 Alice
			10 Bob
			2 Carol
			1 David
	
	decreaseAllowance:
		Alice:
			0 Alice
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
		Alice:
			1 Alice Alice

		Bob:
			8 Alice Carol
			0 Carol David       ${b_premigrate? '': '**fail insufficient allowance'}
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

	---

	${s_prog_xfers}

	---

	${s_prog_vks}
`;

{
	// parse program
	const k_parser = new Parser(program(true));

	// prep evaluator
	const k_evaluator = new Evaluator(H_FUNCTIONS);

	// evaluate program
	await k_evaluator.evaluate(k_parser, k_snip_original);
}

// validate state
async function validate_state(b_premigrate=false) {
	// concurrency
	const kl_queries = MutexPool(2);

	// each eoa in batches
	await Promise.all(a_eoas.map(k_eoa => kl_queries.use(async() => {
		// destructure eoa
		const {
			bank: xg_bank,
			balance: xg_balance,
			address: sa_owner,
			alias: s_alias,
			transfers: a_events,
		} = k_eoa;

		// resolve
		const [[,, g_bank], a4_balance, a4_history] = await Promise.all([
			// query bank module
			b_premigrate? queryCosmosBankBalance(P_SECRET_LCD, sa_owner, 'uscrt'): [],

			// query contract balance
			query_secret_contract(b_premigrate? k_snip_original: k_snip_migrated, 'balance', {
				address: sa_owner,
			}, k_eoa.viewingKey),

			// query transfer history
			b_premigrate
				? query_secret_contract(k_snip_original, 'transfer_history', {
					address: sa_owner,
					page_size: 2048,
				}, k_eoa.viewingKey)
				: query_secret_contract(k_snip_migrated, 'legacy_transfer_history', {
					address: sa_owner,
					page_size: 2048,
				}, k_eoa.viewingKey),
		]);

		// assert bank balances match
		if(b_premigrate && `${xg_bank}` !== g_bank?.balance?.amount) {
			throw Error(`Bank discrepancy for ${s_alias || sa_owner}; suite accounts for ${xg_bank} but bank module reports ${g_bank?.balance?.amount}`);
		}

		// detuple balance
		const [g_balance] = a4_balance;

		// assert that the SNIP balances are identical
		if(`${xg_balance}` !== g_balance?.amount) {
			debugger;
			throw Error(`Balance discrepancy for ${s_alias || sa_owner}; suite accounts for ${xg_balance} but contract reports ${g_balance?.amount}; ${a4_balance}`);
		}

		// detuple history result
		const [g_history] = a4_history as unknown as [Snip20['queries']['transfer_history']['merged']['response']];

		// canonicalize and serialize all transfers for this eoa
		const a_canonical_xfers = k_eoa.transfers.map(g => stringify_json(canonicalize_json(g)));

		// each event in history
		for(const g_tx of g_history.txs) {
			// canonicalize and serialize this transfer
			const si_xfer = stringify_json(canonicalize_json({
				sender: g_tx.sender,
				from: g_tx.from,
				receiver: g_tx.receiver,
				coins: g_tx.coins,
			}));

			// find in list
			const i_xfer = a_canonical_xfers.indexOf(si_xfer);

			// not found
			if(i_xfer < 0) {
				debugger;
				throw Error(`Failed to find transfer event locally`);
			}

			// delete it
			a_canonical_xfers.splice(i_xfer, 1);
		}

		// extra event
		if(a_canonical_xfers.length) {
			throw Error(`Suite recorded transfer event that was not found in contract`);
		}

		// post-migration
		if(!b_premigrate) {
			// canonicalize and serialize all txs for this eoa
			const a_canonical_txs = k_eoa.txs.map(g => stringify_json(canonicalize_json(g)));

			// query tx history
			const a4_history_txs = await query_secret_contract(k_snip_migrated, 'transaction_history', {
				address: k_eoa.address,
				page_size: 2048,
			}, k_eoa.viewingKey);

			// 
			if(!a4_history_txs[0]) {
				debugger;
			}

			// destructure txs
			const [g_txs] = a4_history_txs;

			// each tx
			for(const g_tx of g_txs!.txs) {
				// canonicalize and serialize this tx
				const si_tx = stringify_json(canonicalize_json({
					action: g_tx.action,
					coins: g_tx.coins,
				}));

				// find in list
				const i_canonical = a_canonical_txs.indexOf(si_tx);

				// not found
				if(i_canonical < 0) {
					debugger;
					throw Error(`Failed to find tx event locally`);
				}

				// delete it
				a_canonical_txs.splice(i_canonical, 1);
			}

			// extra event
			if(a_canonical_txs.length) {
				debugger;
				throw Error(`Suite recorded tx event that was not found in contract`);
			}

			// allowances
			{
				const [[g_given], [g_received]] = await Promise.all([
					query_secret_contract(k_snip_migrated, 'allowances_given', {
						address: k_eoa.address,
						owner: k_eoa.address,
						page_size: 2048,
					}, k_eoa.viewingKey),

					query_secret_contract(k_snip_migrated, 'allowances_received', {
						address: k_eoa.address,
						spender: k_eoa.address,
						page_size: 2048,
					}, k_eoa.viewingKey),
				]);

				// define pairs
				const a_pairs = [
					[g_given?.allowances || [], k_eoa.allowancesGiven, 'spender', 'given'],
					[g_received?.allowances || [], k_eoa.allowancesReceived, 'owner', 'received'],
				] as const;

				// each kind
				for(const [a_allowances, h_allowances, si_other, si_which] of a_pairs) {
					// assert numbers match
					if(a_allowances.length !== keys(h_allowances).length) {
						debugger;
						throw Error(`Suite recorded ${keys(h_allowances).length} allowances ${si_which} for ${k_eoa.alias || k_eoa.address} but contract has ${a_allowances.length}`);
					}

					// each allowance given
					for(const g_allowance of a_allowances) {
						// lookup local allowance
						const g_allowance_local = h_allowances[g_allowance[si_other as unknown as keyof typeof g_allowance] as SecretAccAddr];

						// not found
						if(!g_allowance_local) {
							debugger;
							throw Error(`No allowances ${si_which} found for ${k_eoa.alias || k_eoa.address} locally`);
						}

						// destructure local
						const {
							amount: xg_amount,
							expiration: n_expiration,
						} = g_allowance_local;

						if(g_allowance.allowance !== `${xg_amount}`) {
							debugger;
							throw Error(`Different allowance amounts`);
						}

						// check expiration
						if(n_expiration && g_allowance.expiration !== n_expiration) {
							throw Error(`Different allowance expirations`);
						}
					}
				}
			}
		}
	})));

	//
	console.log(`✅ Verified ${a_eoas.length} transfer histories, ${b_premigrate? 'bank balances': 'transaction histories, allowances'} and SNIP balances`);
}

// validate contract state
await validate_state(true);

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
	await migrate_contract(k_snip_original.addr, k_wallet_a, sg_code_id, atu8_msg);

	// override migrated contract code hash
	k_snip_migrated = await SecretContract(P_SECRET_LCD, k_snip_original.addr, null, XC_CONTRACT_CACHE_BYPASS);

	// check balances and verify viewing keys still work
	console.debug(`Validating post-migration state`);
	await validate_state(false);

	// simulate the same load again
	const k_parser = new Parser(`
		migrateLegacyAccount David 1

		transfer Alice 90 Bob

		---

		${program(false)}
	`);

	// prep evaluator
	const k_evaluator = new Evaluator(H_FUNCTIONS);

	// evaluate program
	await k_evaluator.evaluate(k_parser, k_snip_migrated);

	// validate
	await validate_state(false);

	debugger;
}

