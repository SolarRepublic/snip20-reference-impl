import type {Parser} from './parser';
import type {ParseArgsString} from './types';

import type {WeakSecretAccAddr, SecretContract} from '@solar-republic/neutrino';

import {__UNDEFINED, F_IDENTITY, is_number, snake, stringify_json, transform_values, type Dict} from '@blake.regalia/belt';

import {resolve_account_addr} from './util';

type WithArgs<w_return=void> = (g_args: {
	sender: WeakSecretAccAddr;
}) => w_return;

type Handler = {
	params: string;
	handler: WithArgs;
	before?: WithArgs<{
		funds: bigint;
	}> | undefined;
};

export function handler<s_args extends string>(
	s_args: s_args,
	f_handle: (
		g_args: ParseArgsString<s_args> & {
			sender: WeakSecretAccAddr;
		},
	) => void,
	g_hooks?: {
		before?: (g_args: ParseArgsString<s_args> & {
			sender: WeakSecretAccAddr;
		}) => void;
	}
): Handler {
	return {
		params: s_args,
		handler: f_handle as unknown as any,
		before: g_hooks?.before as unknown as any,
	};
}


// type FunctionsDescriptors<
// 	g_input extends Dict<{
// 		args: string;
// 		handle(sa_sender: WeakSecretAccAddr, g_args: Dict<any>): void;
// 	}>,
// > = {
// 	[si_key in keyof g_input]: {
// 		args: g_input[si_key]['args'];
// 		handle(
// 			sa_sender: WeakSecretAccAddr,
// 			g_args: ParseArgsString<g_input[si_key]['args']>
// 		): void;
// 	}
// };


const H_TYPE_CASTERS: Dict<(s_in: string) => any> = {
	string: F_IDENTITY,
	token: s_token => BigInt(s_token.replace(/_/g, '')),
	account: resolve_account_addr,
} satisfies Dict<(s_in: string) => any>;

export class Evaluator<
	h_functions,
> {
	constructor(
		protected _h_functions: Dict<Handler>
	) {

	}

	async evaluate(
		k_parser: Parser,
		k_snip: SecretContract
	) {
		const {_h_functions} = this;

		debugger;
		for(const g_statement of k_parser.program()) {
			// nothing more
			if(!g_statement) break;

			// destructure
			const {
				method: si_method,
				sender: si_sender,
				args: a_args,
			} = g_statement;

			// lookup method
			const g_method = _h_functions[si_method];

			// prep call args
			const g_args_raw: Dict<any> = {};

			// construct args from params
			await Promise.all(g_method.params.split(/\s*,\s*/g).map(async(s_param, i_param) => {
				// split param string
				const [s_name, s_type] = s_param.split(/\s*:\s*/).map(s => s.trim());

				// set arg after casting
				g_args_raw[s_name] = await H_TYPE_CASTERS[s_type](a_args[i_param]);
			}));

			// convert sender
			const sa_sender = await resolve_account_addr(si_sender);

			// build simulation args
			const g_args_sim = {
				...g_args_raw,
				sender: sa_sender,
			};

			// before hook
			const g_extra = g_method.before?.(g_args_sim);

			// construct actual call args
			const g_exec = {
				[snake(si_method)]: transform_values(g_args_raw, z_value => is_number(z_value)? z_value: `${z_value}`),
			};


			// prep execution message
			const [atu8_exec, atu8_nonce] = await k_snip.exec(g_exec, sa_sender, g_extra?.funds? [[`${g_extra.funds}`, 'uscrt']]: __UNDEFINED);

			// call handler
			g_method.handler(g_args_sim);

			console.log(stringify_json(g_statement));
			debugger;
		}
	}
}
