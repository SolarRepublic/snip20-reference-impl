import { SecretApp, sign_secret_query_permit } from "@solar-republic/neutrino";
import { CwUint128 } from "@solar-republic/types";
import { fail } from "./helper";
import { DwbValidator } from "./dwb";
import BigNumber from "bignumber.js";
import { H_ADDRS, N_DECIMALS } from "./constants";
import { Dict, entries, from_entries, values } from "@blake.regalia/belt";

export async function balance(k_app: SecretApp) {
	const g_permit = await sign_secret_query_permit(k_app.wallet, 'snip-balance', [k_app.contract.addr], ['balance']);
	return await k_app.query('balance', {}, g_permit);
}

export async function transfer(
	k_dwbv: DwbValidator,
	xg_amount: bigint,
	k_app_owner: SecretApp,
	k_app_recipient: SecretApp,
) {
	const sa_owner = k_app_owner.wallet.addr;
	const sa_recipient = k_app_recipient.wallet.addr;

	// section header
	console.log(`# Transfer ${BigNumber(xg_amount+'').shiftedBy(-N_DECIMALS).toFixed()} TKN ${H_ADDRS[sa_owner] || sa_owner} => ${H_ADDRS[sa_recipient]}`);

	// query balance of owner and recipient
	const [
		[g_balance_owner_before],
		[g_balance_recipient_before],
	] = await Promise.all([
		balance(k_app_owner),
		balance(k_app_recipient),
	]);

	// execute transfer
	const [g_result, xc_code, sx_res, g_meta, h_events, si_txn] = await k_app_owner.exec('transfer', {
		amount: `${xg_amount}` as CwUint128,
		recipient: sa_recipient,
	}, 100_000n);

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

	// results
	const sg_gas_used = g_meta?.gas_used;
	console.log(`  ⛽️ ${sg_gas_used} gas`);
	console.log(`  ⏹  ${k_dwbv.empty} spaces`);

	const h_gases: Dict<string[]> = {};
	for(const [si_key, a_values] of entries(h_events!)) {
		const m_gas = /^wasm\.gas\.(\w+)\.(\d+)$/.exec(si_key);
		if(m_gas) {
			(h_gases[m_gas[1]] ??= [])[parseInt(m_gas[2]) - 1] = a_values[0];
		}
	}

	for(const a_values of values(h_gases)) {
		let xg_prev = 0n;

		for(let i_value=0; i_value<a_values.length; i_value++) {
			const xg_value = BigInt(a_values[i_value]);
			a_values[i_value] = i_value? `+${xg_value - xg_prev}`: a_values[i_value];
			xg_prev = xg_value;
		}
	}

	console.log(from_entries(entries(h_gases).sort(([si_a], [si_b]) => si_a.localeCompare(si_b))));

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
}