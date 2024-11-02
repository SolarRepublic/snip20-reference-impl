
import type {Wallet, WeakSecretAccAddr} from '@solar-republic/neutrino';

import {sha256, text_to_bytes, type Dict} from '@blake.regalia/belt';

import {k_wallet_a, k_wallet_b, k_wallet_c, k_wallet_d, SecretWallet} from './constants';

const H_ALIASES: Dict<Wallet<'secret'>> = {};

const H_ACCOUNT_VARS: Dict<Wallet<'secret'>> = {
	a: k_wallet_a,
	b: k_wallet_b,
	c: k_wallet_c,
	d: k_wallet_d,
};


export const resolve_account_wallet = async(s_alias: string): Promise<Wallet<'secret'>> => {
	// variable
	if('$' === s_alias[0]) {
		return H_ACCOUNT_VARS[s_alias.slice(1)];
	}

	// resolve alias if exists
	return H_ALIASES[s_alias] ??= await (async() => {
		// generate secret keyhack
		const atu8_sk = await sha256(text_to_bytes(s_alias));

		// create and return wallet
		return SecretWallet(atu8_sk);
	})();
};

export const resolve_account_addr = async(s_alias: string): Promise<WeakSecretAccAddr> => {
	// bech32 addr
	if(s_alias.startsWith('secret1')) return s_alias as WeakSecretAccAddr;

	// alias
	return (await resolve_account_wallet(s_alias)).addr;
};
