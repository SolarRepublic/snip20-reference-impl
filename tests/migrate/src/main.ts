import type {L} from 'ts-toolbelt';

import type {Snip20, SecretAccAddr} from '@solar-republic/contractor';
import type {Wallet} from '@solar-republic/neutrino';
import type {CwUint128, WeakUintStr} from '@solar-republic/types';

import {readFileSync} from 'node:fs';

import {__UNDEFINED, canonicalize_json, keys, MutexPool, stringify_json} from '@blake.regalia/belt';
import {queryCosmosBankBalance} from '@solar-republic/cosmos-grpc/cosmos/bank/v1beta1/query';
import {destructSecretRegistrationKey} from '@solar-republic/cosmos-grpc/secret/registration/v1beta1/msg';
import {querySecretRegistrationTxKey} from '@solar-republic/cosmos-grpc/secret/registration/v1beta1/query';
import {SecretContract, XC_CONTRACT_CACHE_BYPASS, query_secret_contract, SecretWasm, SecretApp, snip24_amino_sign, secret_contract_upload_code} from '@solar-republic/neutrino';

import {k_wallet_a, k_wallet_b, k_wallet_c, k_wallet_d, P_SECRET_LCD, SR_LOCAL_WASM} from './constants';
import {migrate_contract, preload_original_contract} from './contract';
import {bank, bank_send} from './cosmos';
import {ExternallyOwnedAccount} from './eoa';
import {Evaluator} from './evaluator';
import {H_FUNCTIONS, transfer_from} from './functions';
import {G_GLOBAL} from './global';
import {Parser} from './parser';
import {test_dwb} from './test-dwb';


// mainnet contract token address
const SA_MAINNET_SSCRT = 'secret1k0jntykt7e4g3y88ltc60czgjuqdy4c9e8fzek';

// preload sSCRT
const k_snip_original = await preload_original_contract(SA_MAINNET_SSCRT, k_wallet_a);

// create contract eoa
const k_eoa_snip = await ExternallyOwnedAccount.fromAddress(k_snip_original.addr, '$contract');


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

const G_GAS_USED: {
	[si_key in keyof typeof H_FUNCTIONS]: (WeakUintStr | undefined)[];
} = {
	transfer: [],
	send: [],
	transferFrom: [],
	sendFrom: [],
	setViewingKey: [],
	createViewingKey: [],
	increaseAllowance: [],
	decreaseAllowance: [],
	burn: [],
	burnFrom: [],
	migrateLegacyAccount: [],
	deposit: [],
	redeem: [],
};

// evaluates the given program on the given contract
async function evaluate(s_program: string, k_snip: SecretContract) {
	// parse program
	const k_parser = new Parser(s_program);

	// prep evaluator
	const k_evaluator = new Evaluator(H_FUNCTIONS);

	// evaluate program
	await k_evaluator.evaluate(k_parser, k_snip);
}

async function print_genesis_balances() {
	// print genesis account balances
	for(const k_eoa of a_eoas_genesis) {
		// query balance using viewing key
		const [g_balance_vk, xc_code, s_err] = await query_secret_contract(G_GLOBAL.k_snip_migrated ?? k_snip_original, 'balance', {
			address: k_eoa.address,
		}, k_eoa.viewingKey);

		if(!g_balance_vk) {
			debugger;
			throw Error(s_err);
		}

		// genesis account
		console.log(`${k_eoa.alias} balance: ${g_balance_vk?.amount}`);
	}
}

// transfer method
const perform_transfer = async(
	k_wallet: Wallet<'secret'>,
	xg_amount: bigint,
	k_recipient: ExternallyOwnedAccount,
	...a_args: L.Take<Parameters<typeof transfer_from>, 3, '<-'>
) => {
	// create sender eoa
	const k_sender = ExternallyOwnedAccount.at(k_wallet.addr);

	// verbose
	console.log(`${k_sender.alias}: transfer(${xg_amount}, ${k_recipient.alias})`);

	// execute
	const a_results = await SecretApp(k_wallet, G_GLOBAL.k_snip_migrated).exec('transfer', {
		amount: `${xg_amount}` as CwUint128,
		recipient: k_recipient.address,
	}, 800_000n);

	// update locals
	transfer_from(k_sender, k_recipient, xg_amount, ...a_args);

	// return results
	return a_results;
};

// prep batch transfer from method
const perform_batch_transfer_from = async(
	a_actions: [
		k_owner: ExternallyOwnedAccount,
		xg_amount: bigint,
		k_recipient: ExternallyOwnedAccount,
		s_memo?: string,
	][]
) => {
	// verbose
	const a_out: string[] = [];
	for(const [k_owner, xg_amount, k_recipient, s_memo] of a_actions) {
		a_out.push(`${k_owner.alias} -> ${k_recipient.alias} .. ${xg_amount} ${s_memo? 'with memo': ''}`);
	}

	console.log(`$a batch_transfer_from:${a_out.map(s => `\n   ${s}`).join('')}`);

	// execute
	const a_results = await SecretApp(k_wallet_a, G_GLOBAL.k_snip_migrated).exec('batch_transfer_from', {
		actions: a_actions.map(([k_owner, xg_amount, k_recipient, s_memo]) => ({
			owner: k_owner.address,
			amount: `${xg_amount}` as CwUint128,
			recipient: k_recipient.address,
			memo: s_memo,
		})),
	}, 800_000n);

	// update locals
	for(const [k_owner, xg_amount, k_recipient] of a_actions) {
		transfer_from(k_eoa_a, k_recipient, xg_amount, k_owner, true);
	}

	// return results
	return a_results;
};


// genesis accounts
const a_genesis = ['$a', '$b', '$c', '$d'];

// alias accounts
const a_aliases = [
	'Alice',
	'Bob',
	'Carol',
	'David',
	'Zulu',
];

// numeric accounts
const nl_nums = 8;
const a_numerics = Array(nl_nums).fill(0).map((w, i) => `a${i}`);

// instantiate EOAs for all accounts
const a_eoas_genesis = await Promise.all(a_genesis.map(s => ExternallyOwnedAccount.fromAlias(s)));
const a_eoas_aliased = await Promise.all(a_aliases.map(s => ExternallyOwnedAccount.fromAlias(s)));
const a_eoas_numeric = await Promise.all(a_numerics.map(s => ExternallyOwnedAccount.fromAlias(s)));

// concat all eoas
const a_eoas = [...a_eoas_genesis, ...a_eoas_aliased, ...a_eoas_numeric];

// genesis eoas
const [k_eoa_a, k_eoa_b, k_eoa_c, k_eoa_d] = ['$a', '$b', '$c', '$d'].map(si => ExternallyOwnedAccount.at(si));

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

	---

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
			200 Carol
			1 David

	---

	transferFrom:
		Alice:
			1 Alice Alice

		Bob:
			8 Alice Carol
			0 Carol David       ${b_premigrate? '': '**fail insufficient allowance'}
			1000 Alice David    **fail insufficient allowance

		Carol:
			2 Alice David

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

	---
	transfer $a 1 $b
	---
	transfer $a 2 $c
	---
	transfer $d 3 $a
	---
	transfer $b 4 $c
	---
	transfer $a 120 $c
`;



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
		const [[g_bank], a4_balance, a4_history] = await Promise.all([
			// query bank module
			b_premigrate? queryCosmosBankBalance(P_SECRET_LCD, sa_owner, 'uscrt'): [],

			// query contract balance
			query_secret_contract(b_premigrate? k_snip_original: G_GLOBAL.k_snip_migrated, 'balance', {
				address: sa_owner,
			}, k_eoa.viewingKey),

			// query transfer history
			b_premigrate
				? query_secret_contract(k_snip_original, 'transfer_history', {
					address: sa_owner,
					page_size: 2048,
				}, k_eoa.viewingKey)
				: query_secret_contract(G_GLOBAL.k_snip_migrated, 'legacy_transfer_history', {
					address: sa_owner,
					page_size: 2048,
				}, k_eoa.viewingKey),
		]);

		// assert bank balances match
		if(b_premigrate && `${xg_bank}` !== g_bank?.balance?.amount) {
			throw Error(`Bank discrepancy for ${s_alias || sa_owner}; suite accounts for ${xg_bank} but bank module reports ${g_bank?.balance?.amount}`);
		}

		// detuple balance
		const [g_balance, xc_code, s_err] = a4_balance;

		// error
		if(xc_code) {
			debugger;
			throw Error(`Contract query error for ${k_eoa.alias}: ${s_err}`);
		}

		// assert that the SNIP balances are identical
		if(`${xg_balance}` !== g_balance?.amount) {
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
				throw Error('Failed to find transfer event locally');
			}

			// delete it
			a_canonical_xfers.splice(i_xfer, 1);
		}

		// extra event
		if(a_canonical_xfers.length) {
			debugger;
			throw Error(`Suite recorded transfer event that was not found in contract`);
		}

		// post-migration
		if(!b_premigrate) {
			// canonicalize and serialize all txs for this eoa
			const a_canonical_txs = k_eoa.txs.map(g => stringify_json(canonicalize_json(g)));

			// query tx history
			const a4_history_txs = await query_secret_contract(G_GLOBAL.k_snip_migrated, 'transaction_history', {
				address: k_eoa.address,
				page_size: 2048,
			}, k_eoa.viewingKey);

			// 
			if(!a4_history_txs[0]) {
				throw Error('No transaction history');
			}

			// destructure txs
			const [g_txs] = a4_history_txs;

			// each tx
			for(const g_tx of g_txs.txs) {
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
					query_secret_contract(G_GLOBAL.k_snip_migrated, 'allowances_given', {
						address: k_eoa.address,
						owner: k_eoa.address,
						page_size: 2048,
					}, k_eoa.viewingKey),

					query_secret_contract(G_GLOBAL.k_snip_migrated, 'allowances_received', {
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
						throw Error(`Suite recorded ${keys(h_allowances).length} allowances ${si_which} for ${k_eoa.label} but contract has ${a_allowances.length}`);
					}

					// each allowance given
					for(const g_allowance of a_allowances) {
						// lookup local allowance
						const g_allowance_local = h_allowances[g_allowance[si_other as unknown as keyof typeof g_allowance] as SecretAccAddr];

						// not found
						if(!g_allowance_local) {
							throw Error(`No allowances ${si_which} found for ${k_eoa.label} locally`);
						}

						// destructure local
						const {
							amount: xg_amount,
							expiration: n_expiration,
						} = g_allowance_local;

						// check allowance amounts
						if(g_allowance.allowance !== `${xg_amount}`) {
							throw Error(`Different allowance amounts; ${k_eoa.label} locally has ${xg_amount} in allowances ${si_which} to ${ExternallyOwnedAccount.at(g_allowance.spender).alias} but contract reports ${g_allowance.allowance}`);
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


{
	// read WASM file of new contract
	const atu8_wasm = readFileSync(SR_LOCAL_WASM);

	// upload code to chain
	console.debug(`Uploading code...`);
	const [sg_code_id, sb16_codehash, [, s_err]=[]] = await secret_contract_upload_code(k_wallet_a, atu8_wasm, 30_000000n);

	// upload failed
	if(!sg_code_id) {
		throw Error(s_err);
	}

	// prepare migration message
	console.debug('Encoding migration message...');
	const [g_reg] = await querySecretRegistrationTxKey(P_SECRET_LCD);
	const [atu8_cons_pk] = destructSecretRegistrationKey(g_reg!);
	const k_wasm = SecretWasm(atu8_cons_pk!);

	// determine how much uscrt each genesis account actually has
	for(const k_eoa of a_eoas) {
		const [g_bank] = await queryCosmosBankBalance(P_SECRET_LCD, k_eoa.address, 'uscrt');
		bank(k_eoa, BigInt(g_bank?.balance?.amount ?? '0'));
	}

	// fund all aliases
	await bank_send(a_eoas_genesis[0], 5_000000n, a_eoas_aliased);

	// fund first 1024 numeric accounts
	await bank_send(a_eoas_genesis[1], 5_000000n, a_eoas_numeric);

	// sign query permit for all accounts
	await Promise.all(a_eoas.map(async(k_eoa) => {
		k_eoa.queryPermit = await snip24_amino_sign(k_eoa.wallet, 'balance', [k_snip_original.addr], ['owner', 'balance']);
	}));

	// evaluate suite on pre-migrated contract
	await evaluate(`
		${program(true)}

		---

		transfer $a 17 $contract
		transfer $b 16 $contract
		transfer $d 15 $contract
	`, k_snip_original);

	// validate contract state
	await validate_state(true);


	for(const k_eoa of a_eoas_genesis) {
		// query balance using query permit
		const [g_balance_legacy] = await query_secret_contract(k_snip_original, 'balance', {
			address: k_eoa.address,
		}, k_eoa.viewingKey);

		console.log(`${k_eoa.alias} legacy balance: ${g_balance_legacy?.amount}`);
	}

	// // collect gas usage baseline
	// const k_app_original = SecretApp(k_wallet_a, k_snip_original);

	// // collect gas used baselines
	// let xg_used_xfer = 0n;
	// {
	// 	// transfer
	// 	const [, xc_code,, g_meta] = await k_app_original.exec('transfer', {
	// 		recipient: k_wallet_b.addr,
	// 		amount: '1' as CwUint128,
	// 	}, 60_000n);

	// 	if(xc_code) throw Error(`Gas used baseline transfer failed`);
	// 	xg_used_xfer = BigInt(g_meta?.gas_used ?? '0');
	// }

	// verbose
	console.log('## Prior to migration:');
	await print_genesis_balances();

	// run migration
	console.debug(`🏃 Running migration...`);
	await migrate_contract(k_snip_original.addr, k_wallet_a, sg_code_id, k_wasm, sb16_codehash, {
		refund_transfers_to_contract: true,
	});

	// override migrated contract code hash
	G_GLOBAL.k_snip_migrated = await SecretContract(P_SECRET_LCD, k_snip_original.addr, null, __UNDEFINED, XC_CONTRACT_CACHE_BYPASS);

	// verbose
	console.log('## After migration:');
	await print_genesis_balances();

	// expect those accounts to eventually have migration
	k_eoa_a.migrate(true);
	k_eoa_b.migrate(true);
	k_eoa_d.migrate(true);

	// expect genesis accounts to be refunded
	transfer_from(k_eoa_snip, k_eoa_a, 17n, __UNDEFINED, false, true);
	transfer_from(k_eoa_snip, k_eoa_b, 16n, __UNDEFINED, false, true);
	transfer_from(k_eoa_snip, k_eoa_d, 15n, __UNDEFINED, false, true);

	// execute some transfers out in order to settle dwb entries
	await perform_transfer(k_wallet_a, 0n, k_eoa_b, __UNDEFINED, false, true);
	await perform_transfer(k_wallet_b, 0n, k_eoa_d, __UNDEFINED, false, true);
	await perform_transfer(k_wallet_d, 0n, k_eoa_a, __UNDEFINED, false, true);

	// check balances and verify viewing keys still work
	console.debug(`Validating post-migration state`);
	await validate_state(false);

	// expect c to eventually have a migration event
	k_eoa_c.migrate(true);

	// subscribe to notifications on all accounts
	for(const k_eoa of a_eoas) {
		await k_eoa.subscribe(G_GLOBAL.k_snip_migrated);
	}

	// evaluate a new program
	await evaluate(`
		migrateLegacyAccount David 1

		migrateLegacyAccount David _      **fail already migrated

		migrateLegacyAccount Zulu _       **fail legacy balance

		transfer:
			Alice:
				19 Bob
				31 Bob
				5 Carol
				1 David

		transferFrom Carol 1 Alice David  **fail insufficient allowance

		increaseAllowance Alice 50 Carol

		increaseAllowance $a 1000 $a
		increaseAllowance $c 100 $a
		increaseAllowance $d 10 $a

		transfer $a 10 $contract  **fail cannot be sent

		---

		transferFrom Carol 2 Alice David

		---

		${program(false)}

		---

		migrateLegacyAccount $c _

	`, G_GLOBAL.k_snip_migrated);

	// validate
	await validate_state(false);

	// make sure the balance queries are working
	await perform_transfer(k_wallet_a, 1n, k_eoa_b);
	await perform_transfer(k_wallet_a, 2n, k_eoa_c);
	await perform_transfer(k_wallet_b, 3n, k_eoa_c);
	await perform_transfer(k_wallet_b, 4n, k_eoa_c);
	await perform_transfer(k_wallet_c, 5n, k_eoa_a);
	await perform_transfer(k_wallet_c, 6n, k_eoa_a);
	await perform_transfer(k_wallet_a, 7n, k_eoa_c);

	// packet-less multispend on $a, as sender
	// packet-less multirecvd on $b
	// packet-full multirecvd on $c
	// packet-full multispend on $c
	const [g_batch_xfer_1] = await perform_batch_transfer_from([
		[k_eoa_a, 1n, k_eoa_b],
		[k_eoa_a, 2n, k_eoa_c],
		[k_eoa_c, 3n, k_eoa_b],
	]);

	// packet-full mutlirecvd on $a, as sender
	// packet-full multispend on $c (again)
	const [g_batch_xfer_2] = await perform_batch_transfer_from([
		[k_eoa_c, 4n, k_eoa_a],
	]);

	// packet-less multirecvd on $a, as sender
	// packet-less multispend on $c
	const [g_batch_xfer_3] = await perform_batch_transfer_from([
		[k_eoa_c, 5n, k_eoa_a],
		[k_eoa_c, 6n, k_eoa_a],
	]);

	// packet-full multispend on $a, as sender
	const [g_batch_xfer_4] = await perform_batch_transfer_from([
		[k_eoa_a, 7n, k_eoa_d],
	]);

	// packet-full multispend on $a + with memo, as sender
	// packet-full multispend on $c + with memo
	const [g_batch_xfer_5] = await perform_batch_transfer_from([
		[k_eoa_a, 8n, k_eoa_b, 'foo'],
		[k_eoa_c, 9n, k_eoa_d, 'foo'],
	]);

	// failed to perform batch transfer froms
	if(!g_batch_xfer_1 || !g_batch_xfer_2 || !g_batch_xfer_3 || !g_batch_xfer_4 || !g_batch_xfer_5) throw Error(`Batch transfer test failed`);

	// re-validate
	await validate_state(false);

	// check that notifications were verified
	for(const k_eoa of a_eoas) {
		k_eoa.check_notifs();
	}

	// unsubscribe from everything
	for(const k_eoa of a_eoas) {
		k_eoa.unsubscribe();
	}

	const gas = async(s_label: string, f_exec: (sg_target: WeakUintStr) => ReturnType<SecretApp['exec']>, a_targets: WeakUintStr[]) => {
		let i_test = 1;

		for(const sg_target of a_targets) {
			const [w_result, a2_result, [xc_code,,, g_meta, h_events]] = await f_exec(sg_target);

			if(xc_code) throw Error(`Gas used comparison test failed`);

			const xg_used = BigInt(g_meta?.gas_used ?? '0');

			const xg_delta = xg_used - BigInt(sg_target);

			let s_overage = '';
			if('0' !== sg_target) {
				s_overage = `(${xg_delta > 0? `overshot by ${xg_delta} gas`: `${xg_delta} gas UNDER target`})`;
			}

			const sg_check = h_events?.['wasm.check_gas']?.[0] ?? '';

			if(h_events?.['wasm.verify_gas_change']?.[0]) {
				debugger;
			}

			console.log(`Gas used for ${s_label} #${i_test++} w/ evaporation @${sg_target.endsWith('000')? sg_target.replace(/000$/, 'k'): sg_target}: ${xg_used} / ${sg_check} ${s_overage}`);

			await print_genesis_balances();
		}
	};


	// // transfer
	// await gas('transfer', sg_target => k_app_migrated.exec('transfer', {
	// 	recipient: k_wallet_b.addr,
	// 	amount: '1' as CwUint128,
	// 	// gas_target: sg_target,
	// }, 160_000n), [
	// 	'0',
	// 	'76000',
	// 	'77000',
	// 	'100000',
	// ]);

	// // transfer
	// await gas('transfer', sg_target => k_app_migrated.exec('transfer', {
	// 	recipient: k_wallet_c.addr,
	// 	amount: '100' as CwUint128,
	// 	// gas_target: sg_target,
	// }, 160_000n), [
	// 	'0',
	// 	'76000',
	// 	'77000',
	// 	'100000',
	// ]);

	// // transfer
	// await gas('transferFrom', sg_target => k_app_migrated.exec('transfer_from', {
	// 	owner: k_wallet_a.addr,
	// 	recipient: k_wallet_b.addr,
	// 	amount: '1' as CwUint128,
	// 	// gas_target: sg_target,
	// }, 160_000n), [
	// 	'0',
	// 	'80000',
	// 	'81000',
	// 	'100000',
	// ]);


	// // transfer
	// await gas('batchTransferFrom(1)', (sg_target) => k_app_migrated.exec('batch_transfer_from', {
	// 	actions: [
	// 		{
	// 			owner: k_wallet_a.addr,
	// 			recipient: k_wallet_b.addr,
	// 			amount: '1' as CwUint128,
	// 		},
	// 	],
	// 	gas_target: sg_target,
	// }, 160_000n), [
	// 	'0',
	// 	'80000',
	// 	'81000',
	// 	'100000',
	// 	'120000',
	// ]);

	// // transfer
	// await gas('batchTransferFrom(2)', (sg_target) => k_app_migrated.exec('batch_transfer_from', {
	// 	actions: [
	// 		{
	// 			owner: k_wallet_a.addr,
	// 			recipient: k_wallet_b.addr,
	// 			amount: '1' as CwUint128,
	// 		},
	// 		{
	// 			owner: k_wallet_a.addr,
	// 			recipient: k_wallet_b.addr,
	// 			amount: '1' as CwUint128,
	// 		},
	// 	],
	// 	gas_target: sg_target,
	// }, 160_000n), [
	// 	'0',
	// 	'94000',
	// 	'95000',
	// 	'96000',
	// 	'100000',
	// 	'120000',
	// ]);

	// // transfer
	// await gas('batchTransferFrom(3)', (sg_target) => k_app_migrated.exec('batch_transfer_from', {
	// 	actions: [
	// 		{
	// 			owner: k_wallet_a.addr,
	// 			recipient: k_wallet_b.addr,
	// 			amount: '1' as CwUint128,
	// 		},
	// 		{
	// 			owner: k_wallet_a.addr,
	// 			recipient: k_wallet_b.addr,
	// 			amount: '1' as CwUint128,
	// 		},
	// 		{
	// 			owner: k_wallet_a.addr,
	// 			recipient: k_wallet_b.addr,
	// 			amount: '1' as CwUint128,
	// 		},
	// 	],
	// 	gas_target: sg_target,
	// }, 200_000n), [
	// 	'0',
	// 	'107000',
	// 	'108000',
	// 	'109000',
	// 	'120000',
	// ]);


	// print genesis account balances
	for(const k_eoa of a_eoas) {
		// query balance using viewing key
		const [g_balance_vk] = await query_secret_contract(G_GLOBAL.k_snip_migrated, 'balance', {
			address: k_eoa.address,
		}, k_eoa.viewingKey);

		// query balance using query permit
		const [g_balance_qp] = await query_secret_contract(G_GLOBAL.k_snip_migrated, 'balance', {
			address: k_eoa.address,
		}, k_eoa.queryPermit!);

		// balances don't match
		if(g_balance_vk!.amount !== g_balance_qp!.amount) {
			throw Error(`Balance discrepancy between auth modes`);
		}

		// genesis account
		if(a_eoas_genesis.includes(k_eoa)) {
			console.log(`${k_eoa.alias} migrated balance: ${g_balance_vk?.amount}`);
		}
	}

	// dwb tests
	await test_dwb(G_GLOBAL.k_snip_migrated);

	// done
	console.log(`🏁 Finished integrated tests`);
	process.exit(0);
}

