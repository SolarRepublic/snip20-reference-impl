import { snip24_amino_sign } from '@solar-republic/neutrino';
import { entries, stringify_json } from '@blake.regalia/belt';
import { queryCosmosBankBalance } from '@solar-republic/cosmos-grpc/cosmos/bank/v1beta1/query';
import BigNumber from 'bignumber.js';
import { H_ADDRS, N_DECIMALS, P_SECRET_LCD } from './constants.js';
import { fail } from './helper.js';
export async function scrt_balance(sa_owner) {
    const [g_res] = await queryCosmosBankBalance(P_SECRET_LCD, sa_owner, 'uscrt');
    return BigInt(g_res?.balance?.amount || '0');
}
export async function snip_balance(k_app) {
    const g_permit = await snip24_amino_sign(k_app.wallet, 'snip-balance', [k_app.contract.addr], ['balance']);
    return await k_app.query('balance', {}, g_permit);
}
export async function transfer(k_dwbv, xg_amount, k_app_owner, k_app_recipient, k_checker, k_app_sender) {
    const sa_owner = k_app_owner.wallet.addr;
    const sa_recipient = k_app_recipient.wallet.addr;
    // scrt balance of owner before transfer
    // @ts-expect-error canonical addr
    const xg_scrt_balance_owner_before = await scrt_balance(sa_owner);
    // query balance of owner and recipient
    const [[g_balance_owner_before], [g_balance_recipient_before],] = await Promise.all([
        snip_balance(k_app_owner),
        snip_balance(k_app_recipient),
    ]);
    // execute transfer
    const [g_exec, , [xc_code, sx_res, , g_meta, h_events]] = k_app_sender
        ? await k_app_sender.exec('transfer_from', {
            owner: k_app_owner.wallet.addr,
            amount: `${xg_amount}`,
            recipient: sa_recipient,
        }, 250000n)
        : await k_app_owner.exec('transfer', {
            amount: `${xg_amount}`,
            recipient: sa_recipient,
        }, 250000n);
    // section header
    console.log(`# Transfer ${BigNumber(xg_amount + '').shiftedBy(-N_DECIMALS).toFixed()} TKN ${H_ADDRS[sa_owner] || sa_owner}${k_app_sender ? ` (via ${H_ADDRS[k_app_sender.wallet.addr] || k_app_sender.wallet.addr})` : ''} => ${H_ADDRS[sa_recipient] || sa_recipient}      |  ⏹  ${k_dwbv.empty} spaces  |  ⛽️ ${g_meta?.gas_used || '0'} gas used`);
    // query balance of owner and recipient again
    const [[g_balance_owner_after], [g_balance_recipient_after],] = await Promise.all([
        snip_balance(k_app_owner),
        snip_balance(k_app_recipient),
    ]);
    if (xc_code) {
        console.warn('Diagnostics', {
            scrt_balance_before: xg_scrt_balance_owner_before,
            // @ts-expect-error canonical addr
            scrt_balance_after: await scrt_balance(sa_owner),
            snip_balance_before: g_balance_owner_before?.amount,
            snip_balance_after: g_balance_owner_after?.amount,
            meta: stringify_json(g_meta),
            events: h_events,
            exec: g_exec,
        });
        throw Error(`Failed to execute transfer from ${k_app_owner.wallet.addr} [${xc_code}]: ${sx_res}`);
    }
    // sync the buffer
    await k_dwbv.sync();
    const h_tracking = {};
    for (const [si_key, a_values] of entries(h_events)) {
        const m_key = /^wasm\.gas\.(.+)$/.exec(si_key);
        if (m_key) {
            const [, si_group] = m_key;
            const a_logs = [];
            let xg_previous = 0n;
            for (const sx_value of a_values) {
                const [, sg_index, sg_gas, s_comment] = /^(\d+):(\d+):([^]*)$/.exec(sx_value);
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
    if (k_checker) {
        k_checker.compare(h_tracking, BigInt(g_meta.gas_used));
    }
    else if (null === k_checker) {
        console.log(`  ⚖️  Setting baseline gas used to ${g_meta.gas_used}`);
    }
    // prit its state
    k_dwbv.print(true);
    // balance queries failed
    if (!g_balance_owner_before || !g_balance_recipient_before || !g_balance_owner_after || !g_balance_recipient_after) {
        throw fail(`Failed to fetch balances`);
    }
    // expect exact amount difference for owner
    const xg_owner_loss = BigInt(g_balance_owner_before.amount) - BigInt(g_balance_owner_after.amount);
    if (xg_owner_loss !== xg_amount) {
        fail(`Owner's balance changed by ${-xg_owner_loss}, but the amount sent was ${xg_amount}`);
    }
    // expect exact amount difference for recipient
    const xg_recipient_gain = BigInt(g_balance_recipient_after.amount) - BigInt(g_balance_recipient_before.amount);
    if (xg_recipient_gain !== xg_amount) {
        fail(`Recipient's balance changed by ${xg_recipient_gain}, but the amount sent was ${xg_amount}`);
    }
    // make assertions
    k_dwbv.check({
    // shouldNotContainEntriesFor: [k_app_owner.wallet.addr],
    });
    // close
    console.log('\n');
    return {
        tracking: h_tracking,
        gasUsed: BigInt(g_meta.gas_used),
    };
}
//# sourceMappingURL=snip.js.map