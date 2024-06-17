use constant_time_eq::constant_time_eq;
use rand::RngCore;
use secret_toolkit_crypto::ContractPrng;
use serde::{Serialize, Deserialize,};
use serde_big_array::BigArray;
use cosmwasm_std::{to_binary, Api, Binary, CanonicalAddr, StdError, StdResult, Storage};
use secret_toolkit::storage::{AppendStore, Item};

use crate::{gas_tracker::GasTracker, msg::QueryAnswer, state::{safe_add, safe_add_u64, BalancesStore,}, transaction_history::{Tx, TRANSACTIONS}};

pub const KEY_DWB: &[u8] = b"dwb";
pub const KEY_TX_NODES_COUNT: &[u8] = b"dwb-node-cnt";
pub const KEY_TX_NODES: &[u8] = b"dwb-tx-nodes";
pub const KEY_ACCOUNT_TXS: &[u8] = b"dwb-acc-txs";
pub const KEY_ACCOUNT_TX_COUNT: &[u8] = b"dwb-acc-tx-cnt";

pub static DWB: Item<DelayedWriteBuffer> = Item::new(KEY_DWB);
// use with add_suffix tx id (u64)
// does not need to be an AppendStore because we never need to iterate over global list of txs
pub static TX_NODES: Item<TxNode> = Item::new(KEY_TX_NODES);
pub static TX_NODES_COUNT: Item<u64> = Item::new(KEY_TX_NODES_COUNT);

fn store_new_tx_node(store: &mut dyn Storage, tx_node: TxNode) -> StdResult<u64> {
    // tx nodes ids serialized start at 1
    let tx_nodes_serial_id = TX_NODES_COUNT.load(store).unwrap_or_default() + 1;
    TX_NODES.add_suffix(&tx_nodes_serial_id.to_be_bytes()).save(store, &tx_node)?;
    TX_NODES_COUNT.save(store,&(tx_nodes_serial_id))?;
    Ok(tx_nodes_serial_id)
}

// 64 entries + 1 "dummy" entry prepended (idx: 0 in DelayedWriteBufferEntry array)
// minimum allowable size: 3
pub const DWB_LEN: u16 = 65;

// maximum number of tx events allowed in an entry's linked list
pub const DWB_MAX_TX_EVENTS: u16 = u16::MAX;

#[derive(Serialize, Deserialize, Debug)]
pub struct DelayedWriteBuffer {
    pub empty_space_counter: u16,
    #[serde(with = "BigArray")]
    pub entries: [DelayedWriteBufferEntry; DWB_LEN as usize],
}

#[inline]
fn random_addr(rng: &mut ContractPrng) -> CanonicalAddr {
    #[cfg(test)]
    return CanonicalAddr::from(&[rng.rand_bytes(), rng.rand_bytes()].concat()[0..DWB_RECIPIENT_BYTES]); // because mock canonical addr is 54 bytes
    #[cfg(not(test))]
    CanonicalAddr::from(&rng.rand_bytes()[0..DWB_RECIPIENT_BYTES]) // canonical addr is 20 bytes (less than 32)
}

pub fn random_in_range(rng: &mut ContractPrng, a: u32, b: u32) -> StdResult<u32> {
    if b <= a {
        return Err(StdError::generic_err("invalid range"));
    }
    let range_size = (b - a) as u64;
    // need to make sure random is below threshold to prevent modulo bias
    let threshold = u64::MAX - range_size;
    loop {
        // this loop will almost always run only once since range_size << u64::MAX
        let random_u64 = rng.next_u64();
        if random_u64 < threshold { 
            return Ok((random_u64 % range_size) as u32 + a)
        }
    }
}

impl DelayedWriteBuffer {
    pub fn new() -> StdResult<Self> {
        Ok(Self {
            empty_space_counter: DWB_LEN - 1,
            // first entry is a dummy entry for constant-time writing
            entries: [
                DelayedWriteBufferEntry::new(CanonicalAddr::from(&ZERO_ADDR))?; DWB_LEN as usize
            ]
        })
    }

    /// settles an entry at a given index in the buffer
    fn settle_entry(
        &mut self,
        store: &mut dyn Storage,
        index: usize,
    ) -> StdResult<()> {
        let entry = self.entries[index];
        let account = entry.recipient()?;

        AccountTxsStore::append_bundle(
            store,
            &account,
            entry.head_node()?,
            entry.list_len()?,
        )?;

        // get the address' stored balance
        let mut balance = BalancesStore::load(store, &account);
        safe_add(&mut balance, entry.amount()? as u128);
        // add the amount from entry to the stored balance
        BalancesStore::save(store, &account, balance)
    }

    /// settles a participant's account who may or may not have an entry in the buffer
    /// gets balance including any amount in the buffer, and then subtracts amount spent in this tx
    pub fn settle_sender_or_owner_account(
        &mut self,
        store: &mut dyn Storage,
        rng: &mut ContractPrng,
        address: &CanonicalAddr,
        tx_id: u64,
        amount_spent: u128,
        op_name: &str,
    ) -> StdResult<()> {
        // release the address from the buffer
        let (balance, mut entry) = self.constant_time_release(
            store, 
            rng, 
            address
        )?;

        let head_node = entry.add_tx_node(store, tx_id)?;

        AccountTxsStore::append_bundle(
            store,
            address,
            head_node,
            entry.list_len()?,
        )?;
    
        let new_balance = if let Some(balance_after_sub) = balance.checked_sub(amount_spent) {
            balance_after_sub
        } else {
            return Err(StdError::generic_err(format!(
                "insufficient funds to {op_name}: balance={balance}, required={amount_spent}",
            )));
        };
        BalancesStore::save(store, address, new_balance)?;
    
        Ok(())
    }

    /// "releases" a given recipient from the buffer, removing their entry if one exists, in constant-time
    /// returns the new balance and the buffer entry
    fn constant_time_release(
        &mut self, 
        store: &mut dyn Storage, 
        rng: &mut ContractPrng, 
        address: &CanonicalAddr
    ) -> StdResult<(u128, DelayedWriteBufferEntry)> {
        // get the address' stored balance
        let mut balance = BalancesStore::load(store, address);

        // locate the position of the entry in the buffer
        let matched_entry_idx = self.recipient_match(address);

        let replacement_entry = self.unique_random_entry(rng)?;

        // get the current entry at the matched index (0 if dummy)
        let entry = self.entries[matched_entry_idx];
        // add entry amount to the stored balance for the address (will be 0 if dummy)
        safe_add(&mut balance, entry.amount()? as u128);
        // overwrite the entry idx with random addr replacement
        self.entries[matched_entry_idx] = replacement_entry;

        Ok((balance, entry))
    }

    fn unique_random_entry(&self, rng: &mut ContractPrng) -> StdResult<DelayedWriteBufferEntry> {
        // produce a new random address
        let mut replacement_address = random_addr(rng);
        // ensure random addr is not already in dwb (extremely unlikely!!)
        while self.recipient_match(&replacement_address) > 0 {
            replacement_address = random_addr(rng);
        }
        DelayedWriteBufferEntry::new(replacement_address)
    }

    // returns matched index for a given address
    pub fn recipient_match(&self, address: &CanonicalAddr) -> usize {
        let mut matched_index: usize = 0;
        let address = address.as_slice();
        for (idx, entry) in self.entries.iter().enumerate().skip(1) {
            let equals = constant_time_eq(address, entry.recipient_slice()) as usize;
            // an address can only occur once in the buffer
            matched_index |= idx * equals;
        }
        matched_index
    }

    pub fn add_recipient<'a>(
        &mut self,
        store: &mut dyn Storage,
        rng: &mut ContractPrng,
        recipient: &CanonicalAddr,
        tx_id: u64,
        amount: u128,
        tracker: &mut GasTracker<'a>,
    ) -> StdResult<()> {
        let mut group = tracker.group("add_recipient");
        group.log("start");

        // check if `recipient` is already a recipient in the delayed write buffer
        let recipient_index = self.recipient_match(recipient);

        group.log("recipient_match");

        // the new entry will either derive from a prior entry for the recipient or the dummy entry
        let mut new_entry = self.entries[recipient_index].clone();
        new_entry.set_recipient(recipient)?;
        new_entry.add_tx_node(store, tx_id)?;
        new_entry.add_amount(amount)?;

        // whether or not recipient is in the buffer (non-zero index)
        // casting to i32 will never overflow, so long as dwb length is limited to a u16 value
        let if_recipient_in_buffer = constant_time_is_not_zero(recipient_index as i32);

        // randomly pick an entry to exclude in case the recipient is not in the buffer
        let random_exclude_index = random_in_range(rng, 1, DWB_LEN as u32)? as usize;
        //println!("random_exclude_index: {random_exclude_index}");

        // index of entry to exclude from selection
        let exclude_index = constant_time_if_else(if_recipient_in_buffer, recipient_index, random_exclude_index);

        // randomly select any other entry to settle in constant-time (avoiding the reserved 0th position)
        let random_settle_index = (((random_in_range(rng, 0, DWB_LEN as u32 - 2)? + exclude_index as u32) % (DWB_LEN as u32 - 1)) + 1) as usize;
        //println!("random_settle_index: {random_settle_index}");

        // whether or not the buffer is fully saturated yet
        let if_undersaturated = constant_time_is_not_zero(self.empty_space_counter as i32);

        // find the next empty entry in the buffer
        let next_empty_index = (DWB_LEN - self.empty_space_counter) as usize;

        // if buffer is not yet saturated, settle the address at the next empty index
        let bounded_settle_index = constant_time_if_else(if_undersaturated, next_empty_index, random_settle_index);

        // check if we have any open slots in the linked list
        let if_list_can_grow = constant_time_is_not_zero((DWB_MAX_TX_EVENTS - self.entries[recipient_index].list_len()?) as i32);

        // if we would overflow the list, just settle recipient
        // TODO: see docs for attack analysis
        let actual_settle_index = constant_time_if_else(if_list_can_grow, bounded_settle_index, recipient_index);

        // settle the entry
        self.settle_entry(store, actual_settle_index)?;

        // replace it with a randomly generated address (that is not currently in the buffer) and 0 amount and nil events pointer
        let replacement_entry = self.unique_random_entry(rng)?;
        self.entries[actual_settle_index] = replacement_entry;

        // pick the index to where the recipient's entry should be written
        let write_index = constant_time_if_else(if_recipient_in_buffer, recipient_index, actual_settle_index);

        // either updates the existing recipient entry, or overwrites the random replacement entry in the settled index
        self.entries[write_index] = new_entry;

        // decrement empty space counter if it is undersaturated and the recipient was not already in the buffer
        self.empty_space_counter -= constant_time_if_else(
            if_undersaturated,
            constant_time_if_else(if_recipient_in_buffer, 0, 1),
            0
        ) as u16;

        Ok(())
    }

}

const U16_BYTES: usize = 2;
const U64_BYTES: usize = 8;

#[cfg(test)]
const DWB_RECIPIENT_BYTES: usize = 54; // because mock_api creates rando canonical addr that is 54 bytes long
#[cfg(not(test))]
const DWB_RECIPIENT_BYTES: usize = 20;
const DWB_AMOUNT_BYTES: usize = 8;     // Max 16 (u128)
const DWB_HEAD_NODE_BYTES: usize = 5;  // Max 8  (u64)
const DWB_LIST_LEN_BYTES: usize = 2;   // u16

const DWB_ENTRY_BYTES: usize = DWB_RECIPIENT_BYTES + DWB_AMOUNT_BYTES + DWB_HEAD_NODE_BYTES + DWB_LIST_LEN_BYTES;

pub const ZERO_ADDR: [u8; DWB_RECIPIENT_BYTES] = [0u8; DWB_RECIPIENT_BYTES];

/// A delayed write buffer entry consists of the following bytes in this order:
/// 
/// // recipient canonical address
/// recipient - 20 bytes
/// // for sscrt w/ 6 decimals u64 is good for > 18 trillion tokens, far exceeding supply
/// // change to 16 bytes (u128) or other size for tokens with more decimals/higher supply
/// amount    - 8 bytes (u64)
/// // global id for head of linked list of transaction nodes
/// // 40 bits allows for over 1 trillion transactions
/// head_node - 5 bytes
/// // length of list (limited to 65535)
/// list_len  - 2 byte
/// 
/// total: 35 bytes
#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
#[cfg_attr(test, derive(Eq, PartialEq))]
pub struct DelayedWriteBufferEntry(
    #[serde(with = "BigArray")]
    [u8; DWB_ENTRY_BYTES]
);

impl DelayedWriteBufferEntry {
    pub fn new(recipient: CanonicalAddr) -> StdResult<Self> {
        let recipient = recipient.as_slice();
        if recipient.len() != DWB_RECIPIENT_BYTES {
            return Err(StdError::generic_err("dwb: invalid recipient length"));
        }
        let mut result = [0u8; DWB_ENTRY_BYTES];
        result[..DWB_RECIPIENT_BYTES].copy_from_slice(recipient);
        Ok(Self {
            0: result
        })
    }

    fn recipient_slice(&self) -> &[u8] {
        &self.0[..DWB_RECIPIENT_BYTES]
    }

    fn recipient(&self) -> StdResult<CanonicalAddr> {
        let result = CanonicalAddr::try_from(self.recipient_slice())
            .or(Err(StdError::generic_err("Get dwb recipient error")))?;
        Ok(result)
    }

    fn set_recipient(&mut self, val: &CanonicalAddr) -> StdResult<()> {
        let val_slice = val.as_slice();
        if val_slice.len() != DWB_RECIPIENT_BYTES {
            return Err(StdError::generic_err("Set dwb recipient error"));
        }
        self.0[..DWB_RECIPIENT_BYTES].copy_from_slice(val_slice);
        Ok(())
    }

    pub fn amount(&self) -> StdResult<u64> {
        let start = DWB_RECIPIENT_BYTES;
        let end = start + DWB_AMOUNT_BYTES;
        let amount_slice = &self.0[start..end];
        let result = amount_slice
            .try_into()
            .or(Err(StdError::generic_err("Get dwb amount error")))?;
        Ok(u64::from_be_bytes(result))
    }

    fn set_amount(&mut self, val: u64) -> StdResult<()> {
        let start = DWB_RECIPIENT_BYTES;
        let end = start + DWB_AMOUNT_BYTES;
        if DWB_AMOUNT_BYTES != U64_BYTES {
            return Err(StdError::generic_err("Set dwb amount error"));
        }
        self.0[start..end].copy_from_slice(&val.to_be_bytes());
        Ok(())
    }

    pub fn head_node(&self) -> StdResult<u64> {
        let start = DWB_RECIPIENT_BYTES + DWB_AMOUNT_BYTES;
        let end = start + DWB_HEAD_NODE_BYTES;
        let head_node_slice = &self.0[start..end];
        let mut result = [0u8; U64_BYTES];
        if DWB_HEAD_NODE_BYTES > U64_BYTES {
            return Err(StdError::generic_err("Get dwb head node error"));
        }
        result[U64_BYTES - DWB_HEAD_NODE_BYTES..].copy_from_slice(head_node_slice);
        Ok(u64::from_be_bytes(result))
    }

    fn set_head_node(&mut self, val: u64) -> StdResult<()> {
        let start = DWB_RECIPIENT_BYTES + DWB_AMOUNT_BYTES;
        let end = start + DWB_HEAD_NODE_BYTES;
        let val_bytes = &val.to_be_bytes()[U64_BYTES - DWB_HEAD_NODE_BYTES..];
        if val_bytes.len() != DWB_HEAD_NODE_BYTES {
            return Err(StdError::generic_err("Set dwb head node error"));
        }
        self.0[start..end].copy_from_slice(val_bytes);
        Ok(())
    }

    pub fn list_len(&self) -> StdResult<u16> {
        let start = DWB_RECIPIENT_BYTES + DWB_AMOUNT_BYTES + DWB_HEAD_NODE_BYTES;
        let end = start + DWB_LIST_LEN_BYTES;
        let list_len_slice = &self.0[start..end];
        let result = list_len_slice
            .try_into()
            .or(Err(StdError::generic_err("Get dwb list len error")))?;
        Ok(u16::from_be_bytes(result))
    }

    fn set_list_len(&mut self, val: u16) -> StdResult<()> {
        let start = DWB_RECIPIENT_BYTES + DWB_AMOUNT_BYTES + DWB_HEAD_NODE_BYTES;
        let end = start + DWB_LIST_LEN_BYTES;
        if DWB_LIST_LEN_BYTES != U16_BYTES {
            return Err(StdError::generic_err("Set dwb amount error"));
        }
        self.0[start..end].copy_from_slice(&val.to_be_bytes());
        Ok(())
    }

    /// adds a tx node to the linked list
    /// returns: the new head node
    fn add_tx_node(&mut self, store: &mut dyn Storage, tx_id: u64) -> StdResult<u64> {
        let tx_node = TxNode {
            tx_id,
            next: self.head_node()?,
        };

        // store the new node on chain
        let new_node = store_new_tx_node(store, tx_node)?;
        // set the head node to the new node id
        self.set_head_node(new_node)?;
        // increment the node list length
        self.set_list_len(self.list_len()? + 1)?;
        
        Ok(new_node)
    }

    // adds some amount to the total amount for all txs in the entry linked list
    // returns: the new amount
    fn add_amount(&mut self, add_tx_amount: u128) -> StdResult<u64> {
        // change this to safe_add if your coin needs to store amount in buffer as u128 (e.g. 18 decimals)
        let mut amount = self.amount()?;
        let add_tx_amount_u64 = add_tx_amount
            .try_into()
            .or_else(|_| return Err(StdError::generic_err("dwb: deposit overflow")))?;
        safe_add_u64(&mut amount, add_tx_amount_u64);
        self.set_amount(amount)?;

        Ok(amount)
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct TxNode {
    /// transaction id in the TRANSACTIONS list
    pub tx_id: u64,
    /// TX_NODES idx - pointer to the next node in the linked list
    /// 0 if next is null
    pub next: u64,
}

impl TxNode {
    // converts this and following elements in list to a vec of Tx
    pub fn to_vec(&self, store: &dyn Storage, api: &dyn Api) -> StdResult<Vec<Tx>> {
        let mut result = vec![];
        let mut cur_node = Some(self.to_owned());
        while cur_node.is_some() {
            let node = cur_node.unwrap();
            let stored_tx = TRANSACTIONS
                .add_suffix(&node.tx_id.to_be_bytes())
                .load(store)?;
            let tx = stored_tx.into_humanized(api, node.tx_id)?;
            result.push(tx);
            if node.next > 0 {
                let next_node = TX_NODES.add_suffix(&node.next.to_be_bytes()).load(store)?;
                cur_node = Some(next_node);
            } else {
                cur_node = None;
            }
        }

        Ok(result)
    }
}


#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TxBundle {
    /// TX_NODES idx - pointer to the head tx node in the linked list
    pub head_node: u64,
    /// length of the tx node linked list for this element
    pub list_len: u16,
    /// offset of the first tx of this bundle in the history of txs for the account (for pagination)
    pub offset: u32,
}

/// A tx bundle is 1 or more tx nodes added to an account's history.
/// The bundle points to a linked list of transaction nodes, which each reference
/// a transaction record by its global id.
/// used with add_suffix(canonical addr of account)
pub static ACCOUNT_TXS: AppendStore<TxBundle> = AppendStore::new(KEY_ACCOUNT_TXS);

/// Keeps track of the total count of txs for an account (not tx bundles)
/// used with add_suffix(canonical addr of account)
pub static ACCOUNT_TX_COUNT: Item<u32> = Item::new(KEY_ACCOUNT_TX_COUNT);

pub struct AccountTxsStore {}
impl AccountTxsStore {
    /// appends a new tx bundle for an account, called when non-transfer tx occurs or is settled.
    pub fn append_bundle(store: &mut dyn Storage, account: &CanonicalAddr, head_node: u64, list_len: u16) -> StdResult<()> {
        let account_txs_store = ACCOUNT_TXS.add_suffix(account.as_slice());
        let account_txs_len = account_txs_store.get_len(store)?;
        let tx_bundle;
        if account_txs_len > 0 {
            // peek at the last tx bundle added
            let last_tx_bundle = account_txs_store.get_at(store, account_txs_len - 1)?;
            tx_bundle = TxBundle {
                head_node,
                list_len,
                offset: last_tx_bundle.offset + u32::from(last_tx_bundle.list_len),
            };
        } else { // this is the first bundle for the account
            tx_bundle = TxBundle {
                head_node,
                list_len,
                offset: 0,
            };
        }

        // update the total count of txs for account
        let account_tx_count_store = ACCOUNT_TX_COUNT.add_suffix(account.as_slice());
        let account_tx_count = account_tx_count_store.may_load(store)?.unwrap_or_default();
        account_tx_count_store.save(store, &(account_tx_count.saturating_add(u32::from(list_len))))?;

        account_txs_store.push(store, &tx_bundle)
    }

    /// Does a binary search on the append store to find the bundle where the `start_idx` tx can be found.
    /// For a paginated search `start_idx` = `page` * `page_size`.
    /// Returns the bundle index, the bundle, and the index in the bundle list to start at
    pub fn find_start_bundle(store: &dyn Storage, account: &CanonicalAddr, start_idx: u32) -> StdResult<Option<(u32, TxBundle, u32)>> {
        let account_txs_store = ACCOUNT_TXS.add_suffix(account.as_slice());

        let mut left = 0u32;
        let mut right = account_txs_store.get_len(store)?;

        while left <= right {
            let mid = (left + right) / 2;
            let mid_bundle = account_txs_store.get_at(store, mid)?;
            if start_idx >= mid_bundle.offset && start_idx < mid_bundle.offset + (mid_bundle.list_len as u32) {
                // we have the correct bundle
                // which index in list to start at?
                let start_at = (mid_bundle.list_len as u32) - (start_idx - mid_bundle.offset) - 1;
                return Ok(Some((mid, mid_bundle, start_at)));
            } else if start_idx < mid_bundle.offset {
                right = mid - 1;
            } else {
                left = mid + 1;
            }
        }

        Ok(None)
    }
}

#[inline]
fn constant_time_is_not_zero(value: i32) -> u32 {
    (((value | -value) >> 31) & 1) as u32
}

#[inline]
fn constant_time_if_else(condition: u32, then: usize, els: usize) -> usize {
    (then * condition as usize) | (els * (1 - condition as usize))
}

/// FOR TESTING ONLY! REMOVE
pub fn log_dwb(storage: &dyn Storage) -> StdResult<Binary> {
    let dwb = DWB.load(storage)?;
    to_binary(&QueryAnswer::Dwb { dwb: format!("{:?}", dwb) })
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::{testing::*, Binary, Response, Uint128, OwnedDeps};
    use crate::contract::instantiate;
    use crate::msg::{InstantiateMsg, InitialBalance};
    use crate::transaction_history::{append_new_stored_tx, StoredTxAction};

    use super::*;

    fn init_helper(
        initial_balances: Vec<InitialBalance>,
    ) -> (
        StdResult<Response>,
        OwnedDeps<MockStorage, MockApi, MockQuerier>,
    ) {
        let mut deps = mock_dependencies_with_balance(&[]);
        let env = mock_env();
        let info = mock_info("instantiator", &[]);

        let init_msg = InstantiateMsg {
            name: "sec-sec".to_string(),
            admin: Some("admin".to_string()),
            symbol: "SECSEC".to_string(),
            decimals: 8,
            initial_balances: Some(initial_balances),
            prng_seed: Binary::from("lolz fun yay".as_bytes()),
            config: None,
            supported_denoms: None,
        };

        (instantiate(deps.as_mut(), env, info, init_msg), deps)
    }

    #[test]
    fn test_dwb_entry() {
        let (init_result, mut deps) = init_helper(vec![InitialBalance {
            address: "bob".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );
        let env = mock_env();
        let _info = mock_info("bob", &[]);

        let recipient = CanonicalAddr::from(ZERO_ADDR);
        let mut dwb_entry = DelayedWriteBufferEntry::new(recipient).unwrap();
        assert_eq!(dwb_entry, DelayedWriteBufferEntry([0u8; DWB_ENTRY_BYTES]));

        assert_eq!(dwb_entry.recipient().unwrap(), CanonicalAddr::from(ZERO_ADDR));
        assert_eq!(dwb_entry.amount().unwrap(), 0u64);
        assert_eq!(dwb_entry.head_node().unwrap(), 0u64);
        assert_eq!(dwb_entry.list_len().unwrap(), 0u16);

        let canonical_addr = CanonicalAddr::from(&[1u8; DWB_RECIPIENT_BYTES]);
        dwb_entry.set_recipient(&canonical_addr).unwrap();
        dwb_entry.set_amount(1).unwrap();
        dwb_entry.set_head_node(1).unwrap();
        dwb_entry.set_list_len(1).unwrap();

        assert_eq!(dwb_entry.recipient().unwrap(), CanonicalAddr::from(&[1u8; DWB_RECIPIENT_BYTES]));
        assert_eq!(dwb_entry.amount().unwrap(), 1u64);
        assert_eq!(dwb_entry.head_node().unwrap(), 1u64);
        assert_eq!(dwb_entry.list_len().unwrap(), 1u16);

        // first store the tx information in the global append list of txs and get the new tx id
        let storage = deps.as_mut().storage;
        let from = CanonicalAddr::from(&[2u8; 20]);
        let sender = CanonicalAddr::from(&[2u8; 20]);
        let to = CanonicalAddr::from(&[1u8;20]);
        let action = StoredTxAction::transfer(
            from.clone(), 
            sender.clone(), 
            to.clone()
        );
        let tx_id = append_new_stored_tx(storage, &action, 1000u128, "uscrt".to_string(), Some("memo".to_string()), &env.block).unwrap();

        let result = dwb_entry.add_tx_node(storage, tx_id).unwrap();
        assert_eq!(dwb_entry.head_node().unwrap(), result);
    }
}