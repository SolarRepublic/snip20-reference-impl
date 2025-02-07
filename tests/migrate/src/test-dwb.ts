import type {JsonObject, Dict} from '@blake.regalia/belt';
import type {TxMeta, SecretContract, TxResponseTuple} from '@solar-republic/neutrino';
import type {CwUint128} from '@solar-republic/types';

import {sha256, text_to_bytes, bigint_greater} from '@blake.regalia/belt';
import {SI_MESSAGE_TYPE_COSMOS_BANK_MSG_SEND, encodeCosmosBankMsgSend} from '@solar-republic/cosmos-grpc/cosmos/bank/v1beta1/tx';
import {encodeGoogleProtobufAny} from '@solar-republic/cosmos-grpc/google/protobuf/any';
import {SecretApp, Wallet, create_and_sign_tx_direct, broadcast_result} from '@solar-republic/neutrino';

import BigNumber from 'bignumber.js';

import {P_SECRET_LCD, k_wallet_a, X_GAS_PRICE, k_wallet_b, k_wallet_c, k_wallet_d, N_DECIMALS, SI_SECRET_CHAIN, P_SECRET_RPC} from './constants';
import {DwbValidator} from './dwb';
import {GasChecker} from './gas-checker';
import {transfer, type TransferResult} from './snip';

export async function test_dwb(
	k_contract: SecretContract
) {
	const k_app_a = SecretApp(k_wallet_a, k_contract);
	const k_app_b = SecretApp(k_wallet_b, k_contract);
	const k_app_c = SecretApp(k_wallet_c, k_contract);
	const k_app_d = SecretApp(k_wallet_d, k_contract);

	const H_APPS = {
		a: k_app_a,
		b: k_app_b,
		c: k_app_c,
		d: k_app_d,
	};

	// #ts-expect-error validator!
	const k_dwbv = new DwbValidator(k_app_a);

	async function transfer_chain(sx_chain: string) {
		const a_lines = sx_chain.split(/\s*\n+\s*/g).filter(s => s && /^\s*(\d+)/.test(s));

		let k_checker: GasChecker | null = null;

		for(const sx_line of a_lines) {
			const [, sx_amount, si_from, si_to] = /^\s*([\d.]+)(?:\s*TKN)?\s+(\w+)(?:\s+to|\s*[-=]*>+)?\s+(\w+)\s*/.exec(sx_line)!;

			const xg_amount = BigInt(BigNumber(sx_amount).shiftedBy(0).toFixed(0));

			console.log(sx_amount, si_from, si_to);

			// @ts-expect-error secret app
			const g_result = await transfer(k_dwbv, xg_amount, H_APPS[si_from[0].toLowerCase()] as SecretApp, H_APPS[si_to[0].toLowerCase()] as SecretApp, k_checker);

			if(!k_checker) {
				k_checker = new GasChecker(g_result.tracking, g_result.gasUsed);
			}
		}
	}

	// // evaporation
	// if(B_TEST_EVAPORATION) {
	// 	const xg_post_evaporate_buffer = 50_000n;
	// 	const xg_gas_wanted = 150_000n;
	// 	const xg_gas_target = xg_gas_wanted - xg_post_evaporate_buffer;

	// 	const [g_exec, xc_code, sx_res, g_meta, h_events, si_txn] = await k_app_a.exec('transfer', {
	// 		amount: `${500000n}` as CwUint128,
	// 		recipient: k_wallet_b.addr,
	// 		gas_target: `${xg_gas_target}`,
	// 	}, xg_gas_wanted);

	// 	console.log({g_meta});

	// 	if(xc_code) {
	// 		throw Error(`Failed evaporation test: ${sx_res}`);
	// 	}

	// 	const xg_gas_used = BigInt(g_meta?.gas_used || '0');
	// 	if(xg_gas_used < xg_gas_target) {
	// 		throw Error(`Expected gas used to be greater than ${xg_gas_target} but only used ${xg_gas_used}`);
	// 	}
	// 	else if(bigint_abs(xg_gas_wanted, xg_gas_used) > xg_post_evaporate_buffer) {
	// 		throw Error(`Expected gas used to be ${xg_gas_wanted} but found ${xg_gas_used}`);
	// 	}
	// }

	{
		console.log('# Initialized');
		await k_dwbv.sync();
		k_dwbv.print();
		console.log('\n');

		// basic transfers between principals
		await transfer_chain(`
			1 TKN Alec => Brad
			2 TKN Alec => Candice
			5 TKN Alec => Donald
			1 TKN Brad => Candice     -- Brad's entire balance; settles Brad for 1st time
			1 TKN Candice => Donald   -- should accumulate; settles Candice for 1st time
			1 TKN Donald => Alec      -- re-adds Alec to buffer; settles Donald for 1st time
		`);

		// extended transfers between principals
		await transfer_chain(`
			1 TKN Donald => Brad
			1 TKN Donald => Brad      -- exact same transfer repeated
			1 TKN Alec => Brad
			1 TKN Brad => Candice
			1 TKN Alec => Candice
			1 TKN Candice => Brad     -- yet again
		`);

		// gas checker ref
		let k_checker: GasChecker | null = null;

		// grant action from previous simultion
		let f_grant: undefined | (() => Promise<[w_result: JsonObject | undefined, a2_result?: any, a6_response?: TxResponseTuple]>);

		// number of simulations to perform
		const N_SIMULATIONS = Number(process.env['DWB_SIMULATIONS'] || 1025);

		// record maximum gas used for direct transfers
		let xg_max_gas_used_transfer = 0n;

		// simulate many transfers
		for(let i_sim=1; i_sim<=N_SIMULATIONS; i_sim++) {
			const si_receiver = i_sim+'';

			const k_wallet = await Wallet(await sha256(text_to_bytes(si_receiver)), SI_SECRET_CHAIN, P_SECRET_LCD, P_SECRET_RPC, [X_GAS_PRICE, 'uscrt'], 'secret');

			const k_app_sim = SecretApp(k_wallet, k_contract);

			// label
			console.log(`Alec --> ${si_receiver}`);

			// transfer some gas to sim account
			const [atu8_raw, si_txn] = await create_and_sign_tx_direct(k_wallet_b, [
				encodeGoogleProtobufAny(
					SI_MESSAGE_TYPE_COSMOS_BANK_MSG_SEND,
					encodeCosmosBankMsgSend(k_wallet_b.addr, k_wallet.addr, [[`${10_000n}`, 'uscrt']])
				),
			], 50_000n);

			// submit all in parallel
			const [
				g_result_transfer,
				[xc_send_gas, s_err_send_gas],
				[g_res_increase,, [xc_code, s_err]=[]]=[],,
			] = await Promise.all([
				// #ts-expect-error secret app
				transfer(k_dwbv, i_sim % 2? 100n: 200n, k_app_a, k_app_sim, k_checker),
				broadcast_result(k_wallet, atu8_raw, si_txn),
				f_grant?.(),
			] as const);

			// send gas error
			if(xc_send_gas) {
				throw Error(`Failed to transfer gas: ${s_err_send_gas}`);
			}

			// increase allowance error
			if(f_grant && xc_code) {
				debugger;
				throw Error(`Failed to increase allowance: ${s_err}`);
			}

			// approve Alec as spender for future txs
			f_grant = () => k_app_sim.exec('increase_allowance', {
				spender: k_wallet_a.addr,
				amount: `${100n}` as CwUint128,
			}, 60_000n);

			const g_result_xfer = g_result_transfer as unknown as TransferResult;
			if(!k_checker) {
				k_checker = new GasChecker(g_result_xfer.tracking, g_result_xfer.gasUsed);
			}

			xg_max_gas_used_transfer = bigint_greater(xg_max_gas_used_transfer, g_result_xfer.gasUsed);
		}

		// reset checker
		k_checker = null;

		// record maximum gas used for transfer froms
		let xg_max_gas_used_transfer_from = 0n;

		// perform transfer_from
		for(let i_sim=N_SIMULATIONS-2; i_sim>0; i_sim--) {
			const si_owner = i_sim+'';
			const si_recipient = (i_sim - 1)+'';

			const k_wallet_owner = await Wallet(await sha256(text_to_bytes(si_owner)), SI_SECRET_CHAIN, P_SECRET_LCD, P_SECRET_RPC, [X_GAS_PRICE, 'uscrt'], 'secret');
			const k_wallet_recipient = await Wallet(await sha256(text_to_bytes(si_recipient)), SI_SECRET_CHAIN, P_SECRET_LCD, P_SECRET_RPC, [X_GAS_PRICE, 'uscrt'], 'secret');

			const k_app_owner = SecretApp(k_wallet_owner, k_contract);
			const k_app_recipient = SecretApp(k_wallet_recipient, k_contract);

			console.log(`${si_owner} --> ${si_recipient}`);

			// #ts-expect-error secret app
			const g_result = await transfer(k_dwbv, 100n, k_app_owner, k_app_recipient, k_checker, k_app_a);

			if(!k_checker) {
				k_checker = new GasChecker(g_result.tracking, g_result.gasUsed);
			}

			xg_max_gas_used_transfer_from = bigint_greater(xg_max_gas_used_transfer_from, g_result.gasUsed);
		}

		// report
		console.log({
			xg_max_gas_used_transfer,
			xg_max_gas_used_transfer_from,
		});
	}
}
