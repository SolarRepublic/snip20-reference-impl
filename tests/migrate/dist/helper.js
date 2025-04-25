import { is_string, map_entries } from '@blake.regalia/belt';
/* eslint-disable @typescript-eslint/naming-convention */
export const SX_ANSI_RESET = '\x1b[0m';
export const SX_ANSI_DIM_ON = '\x1b[2m';
export const SX_ANSI_UNDERLINE = '\x1b[4m';
export const SX_ANSI_DIM_OFF = '\x1b[22m';
export const SX_ANSI_RED = '\x1b[31m';
export const SX_ANSI_GREEN = '\x1b[32m';
export const SX_ANSI_YELLOW = '\x1b[33m';
export const SX_ANSI_BLUE = '\x1b[34m';
export const SX_ANSI_MAGENTA = '\x1b[35m';
export const SX_ANSI_CYAN = '\x1b[36m';
export const SX_ANSI_WHITE = '\x1b[37m';
export const SX_ANSI_GRAY_BG = '\x1b[100m';
/* eslint-enable */
// polyfill crypto global for node.js env
globalThis.crypto ||= (await import('crypto')).webcrypto;
export function pass(s_test) {
    // eslint-disable-next-line no-console
    console.log(`${SX_ANSI_GREEN}✓${SX_ANSI_RESET} ${s_test}`);
}
function error(s_test, ...a_args) {
    const a_rest = a_args.map(z => is_string(z) ? z : map_entries(z, ([si, w]) => `\n\t${si}: ${w}`).join('\n'));
    console.error(`${s_test}: ${a_rest.join('; ')}`);
}
export function fail(s_test, ...a_args) {
    error(`❌ ${s_test}`, ...a_args);
    throw Error(`Exitting on error`);
}
export function caught(s_test, ...a_args) {
    error(`💀 ${s_test}`, ...a_args);
}
export async function describe(s_group, f_group) {
    const a_results = [];
    await f_group({
        async it(s_test, f_test) {
            try {
                await f_test();
                a_results.push({
                    type: 'pass',
                    name: s_test,
                });
            }
            catch (e_run) {
                a_results.push({
                    type: 'fail',
                    name: s_test,
                    message: e_run.stack || '',
                });
            }
        },
    });
    console.log('');
    console.log(`# ${s_group}\n${'='.repeat(2 + s_group.length)}`);
    for (const g_result of a_results) {
        if ('pass' === g_result.type) {
            pass(g_result.name);
        }
        else {
            fail(g_result.name, g_result.message);
        }
    }
    console.log('');
}
//# sourceMappingURL=helper.js.map