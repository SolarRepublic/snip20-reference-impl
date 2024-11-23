import type {ExternallyOwnedAccount} from './eoa';

import {encodeCosmosBankMsgSend, SI_MESSAGE_TYPE_COSMOS_BANK_MSG_SEND} from '@solar-republic/cosmos-grpc/cosmos/bank/v1beta1/tx';
import {encodeGoogleProtobufAny, type EncodedGoogleProtobufAny} from '@solar-republic/cosmos-grpc/google/protobuf/any';
import {broadcast_result, create_and_sign_tx_direct, exec_fees} from '@solar-republic/neutrino';

import {X_GAS_PRICE} from './constants';
import {K_TEF_LOCAL} from './contract';


export function balance(k_eoa: ExternallyOwnedAccount, xg_amount: bigint): void {
	k_eoa.balance += xg_amount;

	// cannot be negative
	if(k_eoa.balance < 0n) {
		throw Error(`Unexpected negative balance when modifying with ${xg_amount} on ${k_eoa.address}`);
	}
}

export function bank(k_eoa: ExternallyOwnedAccount, xg_amount: bigint): void {
	// console.log(`${k_eoa.alias} <== ${k_eoa.bank} + ${xg_amount} == ${k_eoa.bank + xg_amount}`);

	k_eoa.bank += xg_amount;

	// cannot be negative
	if(k_eoa.bank < 0n) {
		throw Error(`Unexpected negative bank when modifying with ${xg_amount} on ${k_eoa.address}`);
	}
}

export async function bank_send(k_sender: ExternallyOwnedAccount, xg_amount: bigint, a_recipients: ExternallyOwnedAccount[]): Promise<void> {
	const a_msgs: EncodedGoogleProtobufAny[] = [];

	// gas limit for tx
	let sg_limit = 50_000n;

	// seed all accounts with funds for gas
	for(const k_recipient of a_recipients) {
		const atu8_bank = encodeGoogleProtobufAny(
			SI_MESSAGE_TYPE_COSMOS_BANK_MSG_SEND,
			encodeCosmosBankMsgSend(k_sender.address, k_recipient.address, [[`${xg_amount}`, 'uscrt']])
		);

		// add message
		a_msgs.push(atu8_bank);

		// each message incurs extra gas
		sg_limit += 5_500n;

		// adjust sender's balance
		bank(k_sender, -xg_amount);

		// adjust recipient's balance
		bank(k_recipient, xg_amount);
	}

	// compute gas fees
	const a_fees = exec_fees(sg_limit, X_GAS_PRICE);

	// create and sign tx
	const [atu8_raw,, si_txn] = await create_and_sign_tx_direct(k_sender.wallet, a_msgs, a_fees, sg_limit);

	// pay for gas
	bank(k_sender, -BigInt(a_fees[0][0]+''));

	// broadcast to chain
	const [xc_code, sx_res, g_meta, atu8_data, h_events] = await broadcast_result(k_sender.wallet, atu8_raw, si_txn, K_TEF_LOCAL);

	// failed
	if(xc_code) {
		debugger;
		throw Error(g_meta?.log ?? sx_res);
	}
}
