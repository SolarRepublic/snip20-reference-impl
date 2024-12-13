import type {Snip250TxEvent, Snip20TransferEvent} from './types';
import type {WeakSecretAccAddr} from '@solar-republic/types';

import {bigint_lesser, is_number, bigint_greater} from '@blake.regalia/belt';

import {balance, bank} from './cosmos';
import {ExternallyOwnedAccount} from './eoa';
import {handler} from './evaluator';


import {G_GLOBAL} from './global';

const XG_UINT128_MAX = (1n << 128n) - 1n;

let xg_total_supply = 0n;

// native denom
const s_native_denom = 'uscrt';

function transfer_from(
	k_sender: ExternallyOwnedAccount,
	k_recipient: ExternallyOwnedAccount,
	xg_amount: bigint,
	k_from?: ExternallyOwnedAccount
) {
	const k_owner = k_from || k_sender;

	// migrated
	if(G_GLOBAL.k_snip_migrated) {
		// init migration
		k_owner.migrate();
		k_recipient.migrate();

		// create event
		const g_event: Snip250TxEvent = {
			action: {
				transfer: {
					from: (k_owner || k_sender).address,
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
		k_owner.push(g_event);
		k_recipient.push(g_event);

		// *_from action
		if(k_from) {
			// add to sender history as well
			k_sender.migrate();
			k_sender.push(g_event);

			// update allowances (object by ref appleis to both at once)
			const xg_new_allowance = k_owner.allowancesGiven[k_sender.address].amount -= xg_amount;
			if(xg_new_allowance < 0n) {
				throw Error(`${k_sender.label} overspent their allowance from ${k_owner.label} by ${-xg_new_allowance} when transferring ${xg_amount} to ${k_recipient.label}`);
			}
		}
	}
	// legacy
	else {
		// create legacy event
		const g_event: Snip20TransferEvent = {
			from: k_owner.address,
			sender: k_sender.address,
			receiver: k_recipient.address,
			coins: {
				denom: 'TKN',
				amount: `${xg_amount}` as const,
			},
		};

		// add to histories
		k_owner.transfers.push(g_event);
		k_recipient.transfers.push(g_event);

		// add to sender history as well
		if(k_owner !== k_sender) k_sender.transfers.push(g_event);
	}

	// update balances
	balance(k_owner, -xg_amount);
	balance(k_recipient, xg_amount);
}

function set_allowance(
	k_sender: ExternallyOwnedAccount,
	sa_spender: WeakSecretAccAddr,
	xg_amount: bigint,
	n_exp: number,
	si_which: 'increase' | 'decrease'
) {
	const h_given = k_sender.allowancesGiven;
	const k_spender = ExternallyOwnedAccount.at(sa_spender);
	const h_recvd = k_spender.allowancesReceived;
	const sa_sender = k_sender.address;
	const g_prev = h_given[sa_spender];

	if(G_GLOBAL.k_snip_migrated) {
		const xg_allowance = 'increase' === si_which
			? bigint_lesser(XG_UINT128_MAX, (is_number(g_prev?.expiration) && g_prev.expiration < Date.now()? 0n: g_prev?.amount || 0n) + xg_amount)
			: bigint_greater(0n, (is_number(g_prev?.expiration) && g_prev.expiration < Date.now()? 0n: g_prev?.amount || 0n) - xg_amount);

		h_given[sa_spender] = h_recvd[sa_sender] = {
			amount: xg_allowance,
			expiration: n_exp,
		};

		k_spender.check_allowance_notif(k_sender, xg_allowance, n_exp);
	}
}

/**
 * suite handlers for tracking and double checking expected states in contract
 */
export const H_FUNCTIONS = {
	createViewingKey: handler('entropy: string => key: string', (k_sender, g_args, g_answer) => {
		k_sender.viewingKey = g_answer.key;
	}),

	setViewingKey: handler('key: string', (k_sender, g_args) => {
		k_sender.viewingKey = g_args.key;
	}),

	deposit: handler('amount: token', (k_sender, g_args) => {
		// post-migration
		if(G_GLOBAL.k_snip_migrated) {
			k_sender.migrate();

			// add tx to history
			k_sender.push({
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
		if(G_GLOBAL.k_snip_migrated) {
			k_sender.migrate();

			// add tx to history
			k_sender.push({
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

	transfer: handler('amount: token, recipient: account', (k_sender, g_args, _, [,,g_meta]) => {
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
		set_allowance(k_sender, sa_spender, xg_amount, n_exp, 'increase');
	}),

	decreaseAllowance: handler('amount: token, spender: account, expiration: timestamp', (k_sender, {spender:sa_spender, amount:xg_amount, expiration:n_exp}) => {
		set_allowance(k_sender, sa_spender, xg_amount, n_exp, 'decrease');
	}),

	burn: handler('amount: token', (k_sender, g_args) => {
		// post-migration
		if(G_GLOBAL.k_snip_migrated) {
			k_sender.migrate();

			// add tx to history
			k_sender.push({
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
		if(G_GLOBAL.k_snip_migrated) {
			k_sender.migrate();

			// add tx to history
			k_sender.push({
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
		k_sender.migrate(true);
	}),
};

