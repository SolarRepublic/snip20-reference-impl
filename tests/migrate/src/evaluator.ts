import type {Parser, Statement} from './parser';
import type {ParseSignatureString} from './types';
import type {EncodedGoogleProtobufAny} from '@solar-republic/cosmos-grpc/google/protobuf/any';
import type {WeakSecretAccAddr, SecretContract, TxResultTuple, CwSecretAccAddr} from '@solar-republic/neutrino';

import {__UNDEFINED, F_IDENTITY, is_number, is_undefined, keys, parse_json_safe, snake, stringify_json, text_to_base64, transform_values, type Dict, type JsonObject, type Promisable} from '@blake.regalia/belt';
import {broadcast_result, create_and_sign_tx_direct, exec_fees, secret_contract_responses_decrypt} from '@solar-republic/neutrino';

import {X_GAS_PRICE} from './constants';
import {K_TEF_LOCAL} from './contract';
import {ExternallyOwnedAccount} from './eoa';

type WithArgs<a_extra extends Array<any>, w_return=void> = (k_sender: ExternallyOwnedAccount, g_args: {
	sender: WeakSecretAccAddr;
}, ...a_args: a_extra) => w_return;

type Handler = {
	params: string;
	handler: WithArgs<[JsonObject, TxResultTuple]>;
	before?: WithArgs<[], {
		funds: bigint;
	}> | undefined;
};

export function handler<s_signature extends string>(
	s_args: s_signature,
	f_handle: (
		k_sender: ExternallyOwnedAccount,
		g_args: ParseSignatureString<s_signature>['args'] & {
			sender: WeakSecretAccAddr;
		},
		g_answer: ParseSignatureString<s_signature>['return'],
		a_results: TxResultTuple,
	) => void,
	g_hooks?: {
		before?: (k_eoa: ExternallyOwnedAccount, g_args: ParseSignatureString<s_signature>['args'] & {
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

const H_TYPE_CASTERS: Dict<(s_in: string) => any> = {
	string: F_IDENTITY,
	token: s_token => BigInt(s_token.replace(/_/g, '')),
	account: s => ExternallyOwnedAccount.at(s).address,
	timestamp(s_spec) {
		const m_timestamp = /^([+-]?)([\d.]+)([smhd])$/.exec(s_spec);
		if(m_timestamp) {
			const [, s_sign, s_amount, s_unit] = m_timestamp;
			const x_sign = '-' === s_sign? -1: 1;
			return (Date.now() + (+s_amount * x_sign * {
				s: 1e3,
				m: 60e3,
				h: 360e3,
				d: 360e3*24,
			}[s_unit]!)) / 1e3;
		}

		return __UNDEFINED;
	},
	json: s => s? text_to_base64(s): __UNDEFINED,
} satisfies Dict<(s_in: string) => any>;

export class Evaluator {
	protected _hm_pending = new Map<ExternallyOwnedAccount, [
		[
			atu8_exec: EncodedGoogleProtobufAny,
			atu8_nonce: Uint8Array,
			g_args: {
				sender: WeakSecretAccAddr;
			},
			g_operation: Statement['value'],
		][],
		(a_tuple: TxResultTuple, a_msgs: [EncodedGoogleProtobufAny, Uint8Array, {
			sender: WeakSecretAccAddr;
		}, Statement['value']][]) => Promisable<void>,
	]>();

	constructor(
		protected _h_functions: Dict<Handler>
	) {
	}

	async _flush() {
		const {_hm_pending} = this;

		// each pending order
		await Promise.all([..._hm_pending.entries()].map(async([k_eoa, [a_execs, f_handler]]) => {
			const xg_limit = 60_000n + (40_000n * BigInt(a_execs.length));

			// sign transaction
			const [atu8_raw, atu8_signdoc, si_txn] = await create_and_sign_tx_direct(
				k_eoa.wallet,
				a_execs.map(([atu8]) => atu8),
				exec_fees(xg_limit, X_GAS_PRICE),
				xg_limit
			);

			// broadcast
			const a_result = await broadcast_result(
				k_eoa.wallet,
				atu8_raw,
				si_txn,
				K_TEF_LOCAL
			);

			// forward to handler
			await f_handler(a_result, a_execs);
		}));

		// clear
		_hm_pending.clear();

		// break
		console.log('---');
	}

	async evaluate(
		k_parser: Parser,
		k_snip: SecretContract
	) {
		const {_h_functions, _hm_pending} = this;

		let sa_previous: CwSecretAccAddr | '' = '';

		let b_flushed = false;

		for(const g_command of k_parser.program()) {
			// nothing more
			if(!g_command) break;

			// break
			if('break' === g_command.type) {
				// flush
				await this._flush();

				// mark as flushed for next command
				b_flushed = true;

				// next command
				continue;
			}

			// destructure
			const {
				value: g_statement,
				value: {
					method: si_method,
					sender: si_sender,
					args: a_args,
					fail: b_fail,
					error: s_expect,
				},
			} = g_command;

			// lookup method
			const g_method = _h_functions[si_method];

			// prep call args
			const g_args_raw: Dict<any> = {};

			// construct args from params
			await Promise.all(g_method.params.replace(/\s*=>.*$/, '').split(/\s*,\s*/g).map(async(s_param, i_param) => {
				// split param string
				const [s_name, s_type] = s_param.split(/\s*:\s*/).map(s => s.trim());

				// set arg after casting
				try {
					// interpret value
					const z_value = g_args_raw[s_name] = await H_TYPE_CASTERS[s_type](a_args[i_param]);

					// delete key if value is undefined
					if(is_undefined(z_value)) delete g_args_raw[s_name];
				}
				catch(e_cast) {
					debugger;
					throw Error(`While trying to cast to ${s_type} on ${s_name} := ${a_args[i_param]}: ${e_cast}`);
				}
			}));

			// create eoa
			const k_eoa = await ExternallyOwnedAccount.fromAlias(si_sender);

			// ref address
			const sa_sender = k_eoa.address;

			// build simulation args
			const g_args_sim = {
				...g_args_raw,
				sender: sa_sender,
			};

			// before hook
			const g_extra = g_method.before?.(k_eoa, g_args_sim);

			// construct actual call args
			const g_exec = {
				[snake(si_method)]: transform_values(g_args_raw, z_value => is_number(z_value)? z_value: `${z_value}`),
			};


			// prep execution message
			const [atu8_exec, atu8_nonce] = await k_snip.exec(g_exec, sa_sender, g_extra?.funds? [[`${g_extra.funds}`, 'uscrt']]: __UNDEFINED);

			// different sender than previous item or expecting failure
			if(b_flushed || sa_previous !== sa_sender as string || b_fail || !this._hm_pending.size) {
				const b_was_flushed = b_flushed;
				b_flushed = false;

				// sender already in pending or expecting failure; flush previous txs
				if(!b_was_flushed && (_hm_pending.has(k_eoa) || b_fail)) {
					await this._flush();
				}

				// set entry in pending
				_hm_pending.set(k_eoa, [[[atu8_exec, atu8_nonce, g_args_sim, g_statement]], async(a_results, a_execs) => {
					// detuple results
					const [xc_code, sx_res, g_meta, atu8_data, h_events] = a_results;

					// decrypt response from contract
					const [a_error, a_responses] = await secret_contract_responses_decrypt(k_snip, [xc_code, sx_res, g_meta, atu8_data], a_execs.map(([, atu8]) => atu8));

					// failed
					if(xc_code) {
						const [s_error, i_message] = a_error!;

						// for(const [atu8_msg, atu8_nonce_local] of a_execs) {
						// 	console.log(bytes_to_base64(atu8_msg));
						// }

						// expecting failure
						if(b_fail) {
							// does not contain error string
							if(!s_error?.includes(s_expect || '')) {
								throw Error(`Actual error message did not include expected;\n\texpected: ${s_expect}\n\t${s_error}`);
							}

							// OK
							return;
						}
						// not expecting failure
						else {
							// const g_balance = await queryCosmosBankBalance(P_SECRET_LCD, await resolve_account_addr(g_statement.sender), 'uscrt');
							debugger;
							// console.log(g_balance, a_error);

							// // console.error(`Failed to execute: ${stringify_json(a_stmts_local)}\n\t${s_error}`);
							// debugger;
							throw Error(s_error ?? g_meta?.log ?? sx_res);
						}
					}

					// was expecting failure
					if(b_fail) {
						debugger;
						throw Error(`Was expecting tx to fail but it succeeded`);
					}

					// succeeded; each response
					a_responses!.forEach(([a_response], i_msg) => {
						// not a compute response
						if(!a_response) return;

						// detuple answer
						const [, g_answer] = a_response;

						// lookup original args
						const [,, g_args_app, g_operation] = a_execs[i_msg];

						// ref method
						const g_method_local = _h_functions[g_operation.method];

						console.log(`${g_operation.sender}.${i_msg}: ${g_operation.method}(${g_operation.args.join(', ')})`);

						// call handler
						g_method_local.handler(k_eoa, g_args_app, g_answer![keys(g_answer!)[0]] as JsonObject, a_results);
					});
				}]);

				// set previous sender
				sa_previous = sa_sender;
			}
			// same as previous sender
			else {
				// update messages in pending
				_hm_pending.get(k_eoa)![0].push([
					atu8_exec,
					atu8_nonce,
					g_args_sim,
					g_statement,
				]);
			}

			// expecting failure, flush single tx
			if(b_fail) await this._flush();
		}

		// flush final entry
		await this._flush();
	}
}
