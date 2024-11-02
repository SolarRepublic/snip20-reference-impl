import {bytes, entries, try_async, try_sync} from '@blake.regalia/belt';
import {ATU8_SHA256_STARSHELL, bech32_decode} from '@solar-republic/crypto';
import {SecretContract, type WeakSecretAccAddr} from '@solar-republic/neutrino';


import {k_wallet_admin, P_SECRET_LCD} from './constants';
import {Evaluator, handler} from './evaluator';
import {Parser} from './parser';

console.log(process.argv);

const SA_CONTRACT = process.argv[2] as WeakSecretAccAddr;

if(!SA_CONTRACT) throw Error(`Must provide contract address in CLI argument`);

const [, e_decode] = try_sync(() => bech32_decode(SA_CONTRACT));
if(e_decode) throw Error(`Invalid bech32 address: ${SA_CONTRACT};\n${(e_decode as Error)+''}`);


/* syntax:
[action] [sender] [...args]
*/


const H_BALANCES: Record<WeakSecretAccAddr, bigint> = {};
const H_VIEWING_KEYS: Record<WeakSecretAccAddr, string> = {};

function balance(sa_owner: WeakSecretAccAddr, xg_amount: bigint) {
	H_BALANCES[sa_owner] ??= 0n;
	H_BALANCES[sa_owner] += xg_amount;

	// cannot be negative
	if(H_BALANCES[sa_owner] < 0n) {
		throw Error(`Unexpected negative balance when modifying with ${xg_amount} on ${sa_owner}`);
	}
}

const H_FUNCTIONS = {
	createViewingKey: handler('key: string', (g_args) => {

	}),

	setViewingKey: handler('key: string', (g_args) => {
		H_VIEWING_KEYS[g_args.sender] = g_args.key;
	}),

	deposit: handler('amount: token', (g_args) => {
		balance(g_args.sender, g_args.amount);
	}, {
		before: g_args => ({
			funds: g_args.amount,
		}),
	}),

	redeem: handler('amount: token', (g_args) => {
		balance(g_args.sender, -g_args.amount);
	}),

	transfer: handler('amount: token, recipient: account', (g_args) => {
		balance(g_args.sender, -g_args.amount);
		balance(g_args.recipient, g_args.amount);
	}),

	send: handler('amount: token, recipient: account, msg: json', (g_args) => {

	}),
};


// 
// const {
// 	Alice: k_alice,
// } = [
// 	'Alice',
// ].map(async s_alias => await resolve_account_addr(s_alias));

const s_program = `
	deposit $a 100_000_000
	deposit $b 100_000_000

	deposit Alice 100_000_000

	transfer:
		Alice:
			10 Bob       ;; comment
			5  Carol
			1  David
`;


const k_parser = new Parser(s_program);

const k_evaluator = new Evaluator(H_FUNCTIONS);

const [k_snip, e_snip] = await try_async(() => SecretContract(P_SECRET_LCD, SA_CONTRACT, ATU8_SHA256_STARSHELL));
if(e_snip) throw Error(`Failed to instantiate contract: ${(e_snip as Error)+''}`);

await k_evaluator.evaluate(k_parser, k_snip!);

for(const [sa_owner, xg_balance] of entries(H_BALANCES)) {
	console.log(`${sa_owner}: ${xg_balance}`);
}

debugger;

const s_test = `;
	createViewingKey Alice secret

	deposit Alice 10

	multiple:
		Alice:
	
	send:

	setViewingKey

	increaseAllowance
	decreaseAllowance
	transferFrom
	batchTransferFrom
	sendFrom
	batchSendFrom
	batchTransferFrom
	burnFrom
	batchBurnFrom
	mint
	batchMint
	revokePermit
	addSupportedDenoms
	removeSupportedDenoms

	addMinters
	removeMinters
	setMinters
	changeAdmin
	setContractStatus
`;



// transfer recipient:secret1xcj amount:"10000" 
