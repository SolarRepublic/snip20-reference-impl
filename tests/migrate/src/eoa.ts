import type {Snip20TransferEvent, Snip250TxEvent} from './types';
import type {Dict, Nilable} from '@blake.regalia/belt';
import type {CwSecretAccAddr, Wallet, WeakSecretAccAddr} from '@solar-republic/neutrino';
import type {SecretQueryPermit, WeakUintStr} from '@solar-republic/types';

import {sha256, text_to_bytes} from '@blake.regalia/belt';
import {pubkey_to_bech32} from '@solar-republic/crypto';
import {sk_to_pk} from '@solar-republic/neutrino';

import {atu8_sk_a, atu8_sk_b, atu8_sk_c, atu8_sk_d, SecretWallet} from './constants';

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

	protected _atu8_pk33: Uint8Array;
	protected _sa_addr: CwSecretAccAddr;

	bank = 0n;
	balance = 0n;

	viewingKey = '';
	queryPermit: Nilable<SecretQueryPermit> = null;

	allowancesGiven: Record<WeakSecretAccAddr, Allowance> = {};
	allowancesReceived: Record<WeakSecretAccAddr, Allowance> = {};

	transfers: Snip20TransferEvent[] = [];
	txs: Snip250TxEvent[] = [];

	protected constructor(
		protected _atu8_sk: Uint8Array,
		protected _k_wallet: Wallet<'secret'>,
		protected _s_alias: string=''
	) {
		const atu8_pk33 = this._atu8_pk33 = sk_to_pk(_atu8_sk);
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
}
