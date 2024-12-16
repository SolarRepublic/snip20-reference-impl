# sSCRT v2 migration contract

This contract is migrates the original sSCRT SNIP-20 contract to version 2.0

**Original mainnet code id:** `5`
**Original mainnet code hash:** `af74387e276be8874f07bec3a87023ee49b0e7ebe08178c49d0a49c3c98ed60e`

## sSCRT version 2.0

This is an implementation of a [SNIP-20](https://github.com/SecretFoundation/SNIPs/blob/master/SNIP-20.md), [SNIP-21](https://github.com/SecretFoundation/SNIPs/blob/master/SNIP-21.md), [SNIP-22](https://github.com/SecretFoundation/SNIPs/blob/master/SNIP-22.md), [SNIP-23](https://github.com/SecretFoundation/SNIPs/blob/master/SNIP-23.md), [SNIP-24](https://github.com/SecretFoundation/SNIPs/blob/master/SNIP-24.md), [~~SNIP-25~~](https://github.com/SecretFoundation/SNIPs/blob/master/SNIP-25.md), [SNIP-26](https://github.com/SecretFoundation/SNIPs/blob/master/SNIP-26.md), [~~SNIP-50~~](https://github.com/SecretFoundation/SNIPs/blob/master/SNIP-50.md) and [SNIP-52](https://github.com/SecretFoundation/SNIPs/blob/master/SNIP-52.md) compliant token contract.

## Usage examples:

To deposit: ***(This is public)***

```secretcli tx compute execute <contract-address> '{"deposit": {}}' --amount 1000000uscrt --from <account>``` 

To send SSCRT:

```secretcli tx compute execute <contract-address> '{"transfer": {"recipient": "<destination_address>", "amount": "<amount_to_send>"}}' --from <account>```

To set your viewing key: 

```secretcli tx compute execute <contract-address> '{"create_viewing_key": {"entropy": "<random_phrase>"}}' --from <account>```

To check your balance:

```secretcli q compute query <contract-address> '{"balance": {"address":"<your_address>", "key":"your_viewing_key"}}'```

To view your transaction history since migration:

```secretcli q compute query <contract-address> '{"transaction_history": {"address": "<your_address>", "key": "<your_viewing_key>", "page": <optional_page_number>, "page_size": <number_of_transactions_to_return>}}'```

To view your legacy transfer history prior to migration:

```secretcli q compute query <contract-address> '{"legacy_transfer_history": {"address": "<your_address>", "key": "<your_viewing_key>", "page": <optional_page_number>, "page_size": <number_of_transactions_to_return>}}'```

To withdraw: ***(This is public)***

```secretcli tx compute execute <contract-address> '{"redeem": {"amount": "<amount_in_smallest_denom_of_token>"}}' --from <account>```

To view the token contract's configuration:

```secretcli q compute query <contract-address> '{"token_config": {}}'```

To view the deposit/redeem exchange rate:

```secretcli q compute query <contract-address> '{"exchange_rate": {}}'```


## Troubleshooting 

All transactions are encrypted, so if you want to see the error returned by a failed transaction, you need to use the command

`secretcli q compute tx <TX_HASH>`

## Privacy Enhancements

 - All transfers/sends (including batch and *_from) use the delayed write buffer (dwb) to address "spicy printf" storage access pattern attacks.
 - Additionally, a bitwise trie of bucketed entries (dwb) creates dynamic anonymity sets for senders/owners, whose balance must be checked when transferring/sending. It also enhances privacy for recipients.
 - When querying for Transaction History, each event's `id` field returned in responses are deterministically obfuscated by `ChaChaRng(XorBytes(ChaChaRng(actual_event_id), internal_secret)) >> (64 - 53)` for better privacy. Without this, an attacker could deduce the number of events that took place between two transactions.


## SNIP-52: Private Push Notifications

This contract publishes encrypted messages to the event log which carry data intended to notify recipients of actions that affect them, such as token transfer and allowances.

Direct channels:
 - `recvd` -- emitted to a recipient when their account receives funds via one of `transfer`, `send`, `transfer_from`, or `send_from`. The notification data includes the amount, the sender, and the memo length.
 - `spent` -- emitted to an owner when their funds are spent, via one of `transfer`, `send`, `transfer_from` or `send_from`. The notification data includes the amount, the recipient, the owner's new balance, and a few other pieces of information such as memo length, number of actions, and whether the spender was the transaction's sender.
 - `allowance` -- emitted to a spender when some allower account has granted them or modified an existing allowance to spend their tokens, via `increase_allowance` or `decrease_allowance`. The notification data includes the amount, the allower, and the expiration of the allowance.

Group channels:
 - `multirecvd` -- emitted to a group of recipients (up to 16) when a `batch_transfer`, `batch_send`, `batch_transfer_from`, or `batch_send_from` has been executed. Each recipient will receive a packet of data containing the amount they received, the last 8 bytes of the owner's address, and some additional metadata.
 - `multispent` -- emitted to a group of spenders (up to 16) when a `batch_transfer_from`, or `batch_send_from` has been executed. Each spender will receive a packet of data containing the amount that was spent, the last 8 bytes of the recipient's address, and some additional metadata.


## Security Features

 - Transfers to the contract itself will be rejected to prevent accidental loss of funds.
 - The migration allows for a one-time processing of refunding any previous transfers made to the contract itself.
