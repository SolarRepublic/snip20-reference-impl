import { SecretApp, sign_secret_query_permit } from "@solar-republic/neutrino";
import { CwUint128 } from "@solar-republic/types";
import { fail } from "./helper";
import { DwbValidator } from "./dwb";
import BigNumber from "bignumber.js";
import { H_ADDRS, N_DECIMALS } from "./constants";
import { Dict, Nilable, entries, from_entries, values } from "@blake.regalia/belt";
import { GasChecker } from "./gas-checker";

export type GasLog = {
	index: number;
	gas: bigint;
	gap: bigint;
	comment: string;
};

export type GroupedGasLogs = Dict<GasLog[]>;

export type TransferResult = {
	tracking: GroupedGasLogs;
	gasUsed: bigint;
};

export async function balance(k_app: SecretApp) {
	const g_permit = await sign_secret_query_permit(k_app.wallet, 'snip-balance', [k_app.contract.addr], ['balance']);
	return await k_app.query('balance', {}, g_permit);
}

export async function transfer(
	k_dwbv: DwbValidator,
	xg_amount: bigint,
	k_app_owner: SecretApp,
	k_app_recipient: SecretApp,
	k_checker?: Nilable<GasChecker>,
): Promise<TransferResult> {
	const sa_owner = k_app_owner.wallet.addr;
	const sa_recipient = k_app_recipient.wallet.addr;

	// query balance of owner and recipient
	const [
		[g_balance_owner_before],
		[g_balance_recipient_before],
	] = await Promise.all([
		balance(k_app_owner),
		balance(k_app_recipient),
	]);

	// execute transfer
	const [g_exec, xc_code, sx_res, g_meta, h_events, si_txn] = await k_app_owner.exec('transfer', {
		amount: `${xg_amount}` as CwUint128,
		recipient: sa_recipient,
	}, 100_000n);

	if(xc_code) {
		throw Error(sx_res);
	}

	// console.log(h_events);

	// query balance of owner and recipient again
	const [
		[g_balance_owner_after],
		[g_balance_recipient_after],
	] = await Promise.all([
		balance(k_app_owner),
		balance(k_app_recipient),
	]);

	// sync the buffer
	await k_dwbv.sync();

	// // results
	// const sg_gas_used = g_meta?.gas_used;
	// console.log(`  ⏹  ${k_dwbv.empty} spaces`);	

	// section header
	console.log(`# Transfer ${BigNumber(xg_amount+'').shiftedBy(-N_DECIMALS).toFixed()} TKN ${H_ADDRS[sa_owner] || sa_owner} => ${H_ADDRS[sa_recipient]}      |  ⏹  ${k_dwbv.empty} spaces  |  ⛽️ ${g_meta!.gas_used} gas used`);

	const h_tracking: GroupedGasLogs = {};
	for(const [si_key, a_values] of entries(h_events!)) {
		const m_key = /^wasm\.gas\.(\w+)$/.exec(si_key);
		if(m_key) {
			const [, si_group] = m_key;

			const a_logs: GasLog[] = [];
			let xg_previous = 0n;

			for(const sx_value of a_values) {
				const [, sg_index, sg_gas, s_comment] = /^(\d+):(\d+):([^]*)$/.exec(sx_value)!;

				const xg_gas = BigInt(sg_gas);

				a_logs.push({
					index: parseInt(sg_index),
					gas: xg_gas,
					gap: xg_gas - xg_previous,
					comment: s_comment,
				});

				xg_previous = xg_gas;
			}

			h_tracking[si_group] = a_logs.sort((g_a, g_b) => g_a.index - g_b.index);
		}
	}

	// console.log(h_tracking);

	if(k_checker) {
		k_checker.compare(h_tracking, BigInt(g_meta!.gas_used));
	}
	else if(null === k_checker) {
		console.log(`  ⚖️  Setting baseline gas used to ${g_meta!.gas_used}`);
	}

	// prit its state
	await k_dwbv.print(true);


	// balance queries failed
	if(!g_balance_owner_before || !g_balance_recipient_before || !g_balance_owner_after || ! g_balance_recipient_after) {
		throw fail(`Failed to fetch balances`);
	}

	// expect exact amount difference
	const xg_owner_loss = BigInt(g_balance_owner_before.amount) - BigInt(g_balance_owner_after.amount);
	if(xg_owner_loss !== xg_amount) {
		fail(`Owner's balance changed by ${-xg_owner_loss}, but the amount sent was ${xg_amount}`);
	}

	// expect exact amount difference
	const xg_recipient_gain = BigInt(g_balance_recipient_after.amount) - BigInt(g_balance_recipient_before.amount);
	if(xg_recipient_gain !== xg_amount) {
		fail(`Recipient's balance changed by ${xg_recipient_gain}, but the amount sent was ${xg_amount}`);
	}

	// make assertions
	await k_dwbv.check({
		shouldNotContainEntriesFor: [k_app_owner.wallet.addr],
	});

	// close
	console.log('\n');

	return {
		tracking: h_tracking,
		gasUsed: BigInt(g_meta!.gas_used),
	}
}