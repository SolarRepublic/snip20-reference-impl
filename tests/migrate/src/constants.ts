import type {TrustedContextUrl} from '@solar-republic/types';

import {base64_to_bytes, sha256, text_to_bytes} from '@blake.regalia/belt';
import {random_bytes} from '@solar-republic/crypto';
import {Wallet} from '@solar-republic/neutrino';

export const SR_LOCAL_WASM = process.env['CONTRACT_PATH'] || '../../contract.wasm.gz';
export const P_SECRET_LCD = (process.env['SECRET_LCD'] || 'http://localhost:1317') as TrustedContextUrl;
export const P_SECRET_RPC = (process.env['SECRET_RPC'] || 'http://localhost:26656') as TrustedContextUrl;
export const SI_SECRET_CHAIN = process.env['SECRET_CHAIN'] || 'secretdev-1';

export const P_MAINNET_LCD = (process.env['SECRET_MAINNET_LCD'] || 'https://lcd.secret.adrius.starshell.net') as TrustedContextUrl;

export const X_GAS_PRICE = 0.1;

export const XG_LIMIT_TRANSFER_ORIGINAL = 60_000n;

export const SecretWallet = (atu8_sk: Uint8Array): Promise<Wallet<'secret'>> => Wallet(atu8_sk, SI_SECRET_CHAIN, P_SECRET_LCD, P_SECRET_RPC, [X_GAS_PRICE, 'uscrt'], 'secret');

export const [atu8_sk_a, atu8_sk_b, atu8_sk_c, atu8_sk_d] = await Promise.all([
	// '8Ke2frmnGdVPipv7+xh9jClrl5EaBb9cowSUgj5GvrY=',
	// 'buqil+tLeeW7VLuugvOdTmkP3+tUwlCoScPZxeteBPE=',
	// 'UFrCdmofR9iChp6Eg7kE5O3wT+jsOXwJPWwB6kSeuhE=',
	// 'MM/1ZSbT5RF1BnaY6ui/i7yEN0mukGzvXUv+jOyjD0E=',

	'genesis-a',
	'genesis-b',
	'genesis-c',
	'genesis-d',
].map(async sh_seed => await sha256(text_to_bytes(sh_seed))));

// import pre-configured wallets
export const [k_wallet_a, k_wallet_b, k_wallet_c, k_wallet_d] = await Promise.all([
	atu8_sk_a,
	atu8_sk_b,
	atu8_sk_c,
	atu8_sk_d,
].map(atu8 => SecretWallet(atu8)));

export const k_wallet_admin = await SecretWallet(random_bytes(32));

export const H_ADDRS = {
	[k_wallet_a.addr]: 'Alec',
	[k_wallet_b.addr]: 'Brad',
	[k_wallet_c.addr]: 'Candice',
	[k_wallet_d.addr]: 'Donald',
};

export const N_DECIMALS = 6;
