import type {TrustedContextUrl} from '@solar-republic/types';

import {base64_to_bytes} from '@blake.regalia/belt';
import {random_bytes} from '@solar-republic/crypto';
import {Wallet} from '@solar-republic/neutrino';

export const P_SECRET_LCD = (process.env['SECRET_LCD'] || 'http://localhost:1317') as TrustedContextUrl;
export const P_SECRET_RPC = (process.env['SECRET_RPC'] || 'http://localhost:26656') as TrustedContextUrl;
export const SI_SECRET_CHAIN = (process.env['SECRET_CHAIN'] || 'secretdev-1') as TrustedContextUrl;

export const X_GAS_PRICE = 0.1;

export const SecretWallet = (atu8_sk: Uint8Array): Promise<Wallet<'secret'>> => Wallet(atu8_sk, SI_SECRET_CHAIN, P_SECRET_LCD, P_SECRET_RPC, 'secret');

// import pre-configured wallets
export const [k_wallet_a, k_wallet_b, k_wallet_c, k_wallet_d] = await Promise.all([
	'8Ke2frmnGdVPipv7+xh9jClrl5EaBb9cowSUgj5GvrY=',
	'buqil+tLeeW7VLuugvOdTmkP3+tUwlCoScPZxeteBPE=',
	'UFrCdmofR9iChp6Eg7kE5O3wT+jsOXwJPWwB6kSeuhE=',
	'MM/1ZSbT5RF1BnaY6ui/i7yEN0mukGzvXUv+jOyjD0E=',
].map(sb64_sk => SecretWallet(base64_to_bytes(sb64_sk))));

export const k_wallet_admin = await SecretWallet(random_bytes(32));

// export const H_ADDRS = {
// 	[k_wallet_a.addr]: 'Alice',
// 	[k_wallet_b.addr]: 'Bob',
// 	[k_wallet_c.addr]: 'Carol',
// 	[k_wallet_d.addr]: 'David',
// };

export const N_DECIMALS = 6;
