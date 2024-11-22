import type {JsonObject} from '@blake.regalia/belt';
import type {WeakSecretAccAddr} from '@solar-republic/neutrino';
import type { WeakUintStr } from '@solar-republic/types';

type ArgTypeMap = {
	string: string;
	u32: number;
	token: bigint;
	account: WeakSecretAccAddr;
	timestamp: number;
	json: JsonObject;
};

type SplitComma<s_str extends string> = s_str extends `${infer s_pre}, ${infer s_post}`
	? [s_pre, ...SplitComma<s_post>]
	: [s_str];

type ParseArgType<s_args> = s_args extends `${infer s_key}: ${infer s_type}`
	? s_type extends keyof ArgTypeMap
		? {
			[si in s_key]: ArgTypeMap[s_type];
		}
		: {}
	: {};

type ParseArgsList<a_args extends unknown[]> = a_args extends [infer s_arg0, ...infer a_rest]
	? ParseArgType<s_arg0> & ParseArgsList<a_rest>
	: {};

export type ParseArgsString<s_args extends string> = ParseArgsList<SplitComma<s_args>>;

type ParseReturnString<s_return extends string> = s_return extends `{${infer s_struct}}`
	? ParseArgsString<s_struct>
	: ParseArgsString<s_return>;

export type ParseSignatureString<s_sig extends string> = s_sig extends `${infer s_args} => ${infer s_return}`
	? {
		args: ParseArgsString<s_args>;
		return: ParseReturnString<s_return>;
	}
	: {
		args: ParseArgsString<s_sig>;
		return: JsonObject;
	};

export type Snip20TransferEvent = {
	from: WeakSecretAccAddr;
	sender: WeakSecretAccAddr;
	receiver: WeakSecretAccAddr;
	coins: {
		denom: string;
		amount: WeakUintStr;
	};
};

export type Snip250TxEvent = {
	action: {
		transfer: {
			from: WeakSecretAccAddr;
			sender: WeakSecretAccAddr;
			recipient: WeakSecretAccAddr;
		};
	} | {
		mint: {
			minter: WeakSecretAccAddr;
			recipient: WeakSecretAccAddr;
		};
	} | {
		burn: {
			burner: WeakSecretAccAddr;
			owner: WeakSecretAccAddr;
		};
	} | {
		deposit: {};
	} | {
		redeem: {};
	} | {
		migration: {};
	};

	coins: {
		denom: string;
		amount: WeakUintStr;
	};
};
