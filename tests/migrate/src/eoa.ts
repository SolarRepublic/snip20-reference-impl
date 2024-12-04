import type {Snip20TransferEvent, Snip250Action, Snip250TxEvent} from './types';
import type {Dict, Nilable} from '@blake.regalia/belt';

import type {EventUnlistener, Wallet, SecretContract} from '@solar-republic/neutrino';
import type {Snip24QueryPermitSigned, WeakSecretAccAddr, CwSecretAccAddr} from '@solar-republic/types';

import {bytes_to_hex, crypto_random_bytes, entries, keys, remove, sha256, text_to_bytes} from '@blake.regalia/belt';
import {bech32_decode, bech32_encode, pubkey_to_bech32} from '@solar-republic/crypto';
import {subscribe_snip52_channels} from '@solar-republic/neutrino';
import {initWasmSecp256k1} from '@solar-republic/wasm-secp256k1';

import {atu8_sk_a, atu8_sk_b, atu8_sk_c, atu8_sk_d, SecretWallet} from './constants';
import {K_TEF_LOCAL, type MigratedContractInterface} from './contract';

type Allowance = {
	amount: bigint;
	expiration: number;
};

const H_ACCOUNT_VARS: Dict<Uint8Array> = {
	a: atu8_sk_a,
	b: atu8_sk_b,
	c: atu8_sk_c,
	d: atu8_sk_d,
};

const h_cached_aliases: Dict<ExternallyOwnedAccount> = {};
const h_cached_addresses: Dict<ExternallyOwnedAccount> = {};

const Y_SECP256K1 = await initWasmSecp256k1();

export class ExternallyOwnedAccount {
	// create EOA from alias
	static async fromAlias(s_alias: string): Promise<ExternallyOwnedAccount> {
		// check cache or create new
		return h_cached_aliases[s_alias] ??= await (async() => {
			// privte key
			const atu8_sk = '$' === s_alias[0]
				// lookup from vars
				? H_ACCOUNT_VARS[s_alias.slice(1)]
				// generate secret key
				: await sha256(text_to_bytes(s_alias));

			// create wallet
			const k_wallet = await SecretWallet(atu8_sk);

			// create instance
			return h_cached_addresses[k_wallet.addr] ??= new ExternallyOwnedAccount(atu8_sk, k_wallet, s_alias);
		})();
	}

	static at(s_acc: string): ExternallyOwnedAccount {
		return h_cached_addresses[s_acc] || h_cached_aliases[s_acc];
	}

	static async fromAddress(sa_from: WeakSecretAccAddr, s_alias?: string): Promise<ExternallyOwnedAccount> {
		// random secret key
		const atu8_sk = crypto_random_bytes(32);

		// create wallet
		const k_wallet = await SecretWallet(atu8_sk);

		// create and return instance
		return h_cached_addresses[sa_from] = new ExternallyOwnedAccount(atu8_sk, k_wallet, s_alias);
	}

	protected _atu8_pk33: Uint8Array;
	protected _sa_addr: CwSecretAccAddr;
	protected _f_unsubscribe?: EventUnlistener;

	protected _a_skip_recvds: Snip250TxEvent[] = [];
	protected _a_skip_autos: Snip250TxEvent[] = [];

	protected _a_notifs_recvd: {
		amount: bigint;
		sender: WeakSecretAccAddr;
	}[] = [];

	protected _a_notifs_spent: {
		amount: bigint;
		recipient: WeakSecretAccAddr;
	}[] = [];

	protected _a_notifs_allowances: {
		amount: bigint;
		allower: WeakSecretAccAddr;
		expires: number;
	}[] = [];

	bank = 0n;
	balance = 0n;

	viewingKey = '';
	queryPermit: Nilable<Snip24QueryPermitSigned> = null;

	allowancesGiven: Record<WeakSecretAccAddr, Allowance> = {};
	allowancesReceived: Record<WeakSecretAccAddr, Allowance> = {};

	transfers: Snip20TransferEvent[] = [];
	txs: Snip250TxEvent[] = [];

	protected constructor(
		protected _atu8_sk: Uint8Array,
		protected _k_wallet: Wallet<'secret'>,
		protected _s_alias: string=''
	) {
		const atu8_pk33 = this._atu8_pk33 = Y_SECP256K1.sk_to_pk(_atu8_sk);
		this._sa_addr = pubkey_to_bech32(atu8_pk33, 'secret');
	}

	get address(): CwSecretAccAddr {
		return this._sa_addr;
	}

	get publicKey(): Uint8Array {
		return this._atu8_pk33;
	}

	get wallet(): Wallet<'secret'> {
		return this._k_wallet;
	}

	get alias(): string {
		return this._s_alias;
	}

	get label(): string {
		return this._s_alias || this._sa_addr;
	}

	migrate(b_explicit=false): void {
		if(b_explicit && this.txs.length) {
			throw Error(`Explicit migration should not have been allowed`);
		}

		// first tx in post-migration
		if(!this.txs.length && this.transfers.length) {
			// add auto-migrate event
			this.txs.push({
				action: {
					migration: {},
				},
				coins: {
					denom: 'TKN',
					amount: `${this.balance}`,
				},
			});
		}
	}

	push(g_event: Snip250TxEvent): void {
		const {
			_a_skip_recvds,
			_a_skip_autos,
			_a_notifs_recvd,
			_a_notifs_spent,
		} = this;

		const si_action = keys(g_event.action)[0];
		const g_action = (g_event.action as Snip250Action)[si_action];

		const xg_amount = BigInt(g_event.coins.amount);

		// transfer action
		if('transfer' === si_action) {
			const g_xfer = g_action as NonNullable<Snip250Action['transfer']>;

			// this account was recipient
			if(this.address === g_xfer.recipient && !_a_skip_recvds.includes(g_event) && !_a_skip_autos.includes(g_event)) {
				if(!_a_notifs_recvd.length) {
					debugger;
					throw Error(`No received notifications`);
				}

				// find notification
				const i_recvd = _a_notifs_recvd.findIndex(g => g_xfer.from === g.sender && xg_amount === g.amount);
				if(i_recvd < 0) {
					debugger;
					throw Error(`Missing received notification`);
				}

				// delete it
				_a_notifs_recvd.splice(i_recvd, 1);

				// am also owner; add event as skip
				if(this.address === g_xfer.from) _a_skip_recvds.push(g_event);
			}
			// this account was owner
			else if(this.address === g_xfer.from && !_a_skip_autos.includes(g_event)) {
				// remove skip if present
				remove(_a_skip_recvds, g_event);

				// find notification
				const i_spent = _a_notifs_spent.findIndex(g => g_xfer.recipient === g.recipient && xg_amount === g.amount);
				if(i_spent < 0) {
					debugger;
					throw Error(`Missing spent notification`);
				}

				// delete it
				_a_notifs_spent.splice(i_spent, 1);

				// am also sender; add event as skip
				if(this.address === g_xfer.sender) _a_skip_autos.push(g_event);
			}
		}


		this.txs.push(g_event);
	}

	check_allowance_notif(k_sender: ExternallyOwnedAccount, xg_amount: bigint, n_exp=0) {
		const {
			_a_notifs_allowances,
		} = this;

		const i_allowance = _a_notifs_allowances.findIndex(g => g.amount === xg_amount && g.allower === k_sender.address && n_exp === g.expires);
		if(i_allowance < 0) {
			debugger;
			throw Error(`Missing allowance notification`);
		}

		// delete
		_a_notifs_allowances.splice(i_allowance, 1);
	}

	check_notifs(): void {
		if(this._a_notifs_recvd.length) {
			throw Error(`Unverified received notification`);
		}
		else if(this._a_notifs_spent.length) {
			throw Error(`Unverified spent notification`);
		}
		else if(this._a_notifs_allowances.length) {
			throw Error(`Unverified allowance notifications`);
		}
	}

	async subscribe(k_snip: SecretContract<MigratedContractInterface>): Promise<void> {
		this._f_unsubscribe = await subscribe_snip52_channels(K_TEF_LOCAL, k_snip, this.queryPermit!, {
			recvd: ([xg_amount, atu8_sender]) => {
				const k_sender = ExternallyOwnedAccount.at(bech32_encode('secret', atu8_sender));

				this._a_notifs_recvd.push({
					amount: xg_amount,
					sender: k_sender.address,
				});

				console.log(`🔔 ${this.label} received ${xg_amount} TKN from ${k_sender.label}`);
			},

			spent: ([xg_amount, n_actions, atu8_recipient, xg_balance]) => {
				const k_recipient = ExternallyOwnedAccount.at(bech32_encode('secret', atu8_recipient));

				const s_outgoing = 1 === n_actions
					? `to ${k_recipient.label}`
					: `in ${n_actions} with first recipient being ${k_recipient.label}`;

				this._a_notifs_spent.push({
					amount: xg_amount,
					recipient: k_recipient.address,
				});

				console.log(`🔔 ${this.label} spent ${xg_amount} TKN ${s_outgoing}; new balance: ${xg_balance}`);
			},

			allowance: ([xg_amount, atu8_allower, n_expiration]) => {
				const k_allower = ExternallyOwnedAccount.at(bech32_encode('secret', atu8_allower));

				const s_expires = n_expiration
					? `that expires ${new Date(n_expiration * 1e3).toISOString()}`
					: `that never expires`;

				this._a_notifs_allowances.push({
					amount: xg_amount,
					allower: k_allower.address,
					expires: n_expiration,
				});

				console.log(`🔔 ${this.label} received an allowance for ${xg_amount} TKN from ${k_allower.label} ${s_expires}`);
			},

			multirecvd: (a_packet, atu8_data, g_tx, h_events) => {
				const k_sender = ExternallyOwnedAccount.at(h_events['message.sender'][0]);

				if(a_packet) {
					const [xg_flags, xg_amount, atu8_term] = a_packet;

					const s0x_sender = bytes_to_hex(atu8_term);

					let s_from = '';

					// sender is owner
					if(xg_flags & 0x80n) {
						s_from = k_sender.label;
					}
					else {
						const a_matches: ExternallyOwnedAccount[] = [];

						for(const [sa_addr, k_eoa] of entries(h_cached_addresses)) {
							if(s0x_sender === bytes_to_hex(bech32_decode(sa_addr).subarray(-8))) {
								a_matches.push(k_eoa);
							}
						}

						s_from = `...${s0x_sender}:[${a_matches.map(k => k.label).join(', ')}]`;
					}

					console.log(`🔔 ${this.label} received multi-recipient transfer for ${xg_amount} TKN from ${s_from} ${xg_flags & 0x40n? `WITH memo`: 'no memo'} executed by ${k_sender.label}`);
				}
				else {
					console.log(`🔔 ${this.label} received multi-recipient transfer for (?) TKN executed by ${k_sender.label}`);
				}

				debugger;
			},

			multispent: (a_spent, atu8_data, g_tx, h_events) => {
				const k_sender = ExternallyOwnedAccount.at(h_events['message.sender'][0]);

				if(a_spent) {
					debugger
					// const [] = a_spent;
					console.log(`🔔 ${this.label} was notified of a multi-spend for (?) TKN executed by ${k_sender.label}`);
				}
				else {
					console.log(`🔔 ${this.label} was notified of a multi-spend for (?) TKN executed by ${k_sender.label}`);
				}
				debugger;
			},
		}, this);
	}

	unsubscribe(): void {
		this._f_unsubscribe?.();
	}
}
