import type {JsonObject} from '@blake.regalia/belt';
import type {WeakSecretAccAddr} from '@solar-republic/neutrino';

type ArgTypeMap = {
	string: string;
	u32: number;
	token: bigint;
	account: WeakSecretAccAddr;
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
