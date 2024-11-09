import type {CwSecretAccAddr, Wallet, WeakSecretAccAddr} from '@solar-republic/neutrino';
import type {SecretQueryPermit} from '@solar-republic/types';

import {sha256, text_to_bytes, type Dict, type Nilable} from '@blake.regalia/belt';
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
				// generate secret key
				? await sha256(text_to_bytes(s_alias))
				// lookup from vars
				: H_ACCOUNT_VARS[s_alias];

			// create wallet
			const k_wallet = await SecretWallet(atu8_sk);

			// create instance
			return h_cached_addresses[k_wallet.addr] ??= new ExternallyOwnedAccount(atu8_sk, k_wallet);
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
}
