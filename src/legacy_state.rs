use std::any::type_name;
use std::convert::TryFrom;

use cosmwasm_std::{
    Api, CanonicalAddr, Coin, Addr, StdError, StdResult, Storage, Uint128,
};
use cosmwasm_storage::{PrefixedStorage, ReadonlyPrefixedStorage};
use secret_toolkit::storage::Item;

use crate::{legacy_append_store::AppendStore, legacy_viewing_key}; //TypedStore, TypedStoreMut};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::msg::{status_level_to_u8, u8_to_status_level, ContractStatusLevel};
//use crate::viewing_key::ViewingKey;
use serde::de::DeserializeOwned;

pub static CONFIG_KEY: &[u8] = b"config";
pub const PREFIX_TXS: &[u8] = b"transfers";

pub const KEY_CONSTANTS: &[u8] = b"constants";
pub const KEY_TOTAL_SUPPLY: &[u8] = b"total_supply";
pub const KEY_CONTRACT_STATUS: &[u8] = b"contract_status";
pub const KEY_MINTERS: &[u8] = b"minters";
pub const KEY_TX_COUNT: &[u8] = b"tx-count";
pub const KEY_VK_SEED: &[u8] = b"vk::seed";

pub static VKSEED: Item<Vec<u8>> = Item::new(KEY_VK_SEED);

pub const PREFIX_CONFIG: &[u8] = b"config";
pub const PREFIX_BALANCES: &[u8] = b"balances";
pub const PREFIX_ALLOWANCES: &[u8] = b"allowances";
pub const PREFIX_VIEW_KEY: &[u8] = b"viewingkey";
pub const PREFIX_RECEIVERS: &[u8] = b"receivers";

// Note that id is a globally incrementing counter.
// Since it's 64 bits long, even at 50 tx/s it would take
// over 11 billion years for it to rollback. I'm pretty sure
// we'll have bigger issues by then.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct Tx {
    pub id: u64,
    pub from: Addr,
    pub sender: Addr,
    pub receiver: Addr,
    pub coins: Coin,
}

impl Tx {
    pub fn into_stored<A: Api>(self, api: &A) -> StdResult<StoredTx> {
        let tx = StoredTx {
            id: self.id,
            from: api.addr_canonicalize(self.from.as_str())?,
            sender: api.addr_canonicalize(self.sender.as_str())?,
            receiver: api.addr_canonicalize(self.receiver.as_str())?,
            coins: self.coins,
        };
        Ok(tx)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredTx {
    pub id: u64,
    pub from: CanonicalAddr,
    pub sender: CanonicalAddr,
    pub receiver: CanonicalAddr,
    pub coins: Coin,
}

impl StoredTx {
    pub fn into_humanized(self, api: &dyn Api) -> StdResult<Tx> {
        let tx = Tx {
            id: self.id,
            from: api.addr_humanize(&self.from)?,
            sender: api.addr_humanize(&self.sender)?,
            receiver: api.addr_humanize(&self.receiver)?,
            coins: self.coins,
        };
        Ok(tx)
    }
}

pub fn get_old_transfers(
    api: &dyn Api,
    storage: &dyn Storage,
    for_address: &CanonicalAddr,
    page: u32,
    page_size: u32,
) -> StdResult<Vec<Tx>> {
    let store = ReadonlyPrefixedStorage::multilevel(storage, &[PREFIX_TXS, for_address.as_slice()]);

    // Try to access the storage of txs for the account.
    // If it doesn't exist yet, return an empty list of transfers.
    let store = if let Some(result) = AppendStore::<StoredTx, _>::attach(&store) {
        result?
    } else {
        return Ok(vec![]);
    };

    // Take `page_size` txs starting from the latest tx, potentially skipping `page * page_size`
    // txs from the start.
    let tx_iter = store
        .iter()
        .rev()
        .skip((page * page_size) as _)
        .take(page_size as _);
    // The `and_then` here flattens the `StdResult<StdResult<Tx>>` to an `StdResult<Tx>`
    let txs: StdResult<Vec<Tx>> = tx_iter
        .map(|tx| tx.map(|tx| tx.into_humanized(api)).and_then(|x| x))
        .collect();
    txs
}

// Config

#[derive(Serialize, Debug, Deserialize, Clone, PartialEq, JsonSchema)]
pub struct Constants {
    pub name: String,
    pub admin: Addr,
    pub symbol: String,
    pub decimals: u8,
    pub prng_seed: Vec<u8>,
    // privacy configuration
    pub total_supply_is_public: bool,
}

fn get_bin_data<T: DeserializeOwned>(storage: &dyn Storage, key: &[u8]) -> StdResult<T> {
    let bin_data = storage.get(key);

    match bin_data {
        None => Err(StdError::not_found("Key not found in storage")),
        Some(bin_data) => Ok(bincode2::deserialize::<T>(&bin_data)
            .map_err(|e| StdError::serialize_err(type_name::<T>(), e))?),
    }
}

pub fn get_old_constants(storage: &dyn Storage) -> StdResult<Constants> {
	let config_storage = ReadonlyPrefixedStorage::new(storage, PREFIX_CONFIG);

	let consts_bytes = config_storage
		.get(KEY_CONSTANTS)
		.ok_or_else(|| StdError::generic_err("no constants stored in configuration"))?;
	bincode2::deserialize::<Constants>(&consts_bytes)
		.map_err(|e| StdError::serialize_err(type_name::<Constants>(), e))
}

pub fn get_old_total_supply(storage: &dyn Storage) -> u128 {
	let config_storage = ReadonlyPrefixedStorage::new(storage, PREFIX_CONFIG);

    // :: total supply
    let supply_bytes = config_storage
        .get(KEY_TOTAL_SUPPLY)
        .expect("no total supply stored in config");
    // This unwrap is ok because we know we stored things correctly
    slice_to_u128(&supply_bytes).unwrap()
}

pub fn get_old_contract_status(storage: &dyn Storage) -> u8 {
	let config_storage = ReadonlyPrefixedStorage::new(storage, PREFIX_CONFIG);

	let status_bytes = config_storage
		.get(KEY_CONTRACT_STATUS)
		.expect("no contract status stored in config");

	// These unwraps are ok because we know we stored things correctly
	slice_to_u8(&status_bytes).unwrap()
}

pub fn get_old_minters(storage: &dyn Storage) -> Vec<Addr> {
	get_bin_data(storage, KEY_MINTERS).unwrap_or_default()
}

pub fn get_old_tx_count(storage: &dyn Storage) -> u64 {
	get_bin_data(storage, KEY_TX_COUNT).unwrap_or_default()
}

// Balances

pub fn get_old_balance(storage: &dyn Storage, account: &CanonicalAddr) -> Option<u128> {
	let balance_storage = ReadonlyPrefixedStorage::new(storage, PREFIX_BALANCES);
	let account_bytes = account.as_slice();
	let result = balance_storage.get(account_bytes);
	match result {
		// This unwrap is ok because we know we stored things correctly
		Some(balance_bytes) => Some(slice_to_u128(&balance_bytes).unwrap()),
		None => None,
	}
}

pub fn clear_old_balance(storage: &mut dyn Storage, account: &CanonicalAddr) {
	let mut balances_store = PrefixedStorage::new(storage, PREFIX_BALANCES);
    balances_store.remove(account.as_slice());
}

// Viewing Keys

pub fn write_viewing_key(store: &mut dyn Storage, owner: &CanonicalAddr, key: &legacy_viewing_key::ViewingKey) {
    let mut viewing_key_store = PrefixedStorage::new(store, PREFIX_VIEW_KEY);
    viewing_key_store.set(owner.as_slice(), &key.to_hashed());
}

pub fn read_viewing_key(store: &dyn Storage, owner: &CanonicalAddr) -> Option<Vec<u8>> {
    let viewing_key_store = ReadonlyPrefixedStorage::new(store, PREFIX_VIEW_KEY);
    viewing_key_store.get(owner.as_slice())
}

// Receiver Interface

pub fn get_receiver_hash(
    store: &dyn Storage,
    account: &Addr,
) -> Option<StdResult<String>> {
    let store = ReadonlyPrefixedStorage::new(store, PREFIX_RECEIVERS);
    store.get(account.as_str().as_bytes()).map(|data| {
        String::from_utf8(data)
            .map_err(|_err| StdError::invalid_utf8("stored code hash was not a valid String"))
    })
}

pub fn set_receiver_hash(store: &mut dyn Storage, account: &Addr, code_hash: String) {
    let mut store = PrefixedStorage::new(store, PREFIX_RECEIVERS);
    store.set(account.as_str().as_bytes(), code_hash.as_bytes());
}

// Helpers

/// Converts 16 bytes value into u128
/// Errors if data found that is not 16 bytes
pub fn slice_to_u128(data: &[u8]) -> StdResult<u128> {
    match <[u8; 16]>::try_from(data) {
        Ok(bytes) => Ok(u128::from_be_bytes(bytes)),
        Err(_) => Err(StdError::generic_err(
            "Corrupted data found. 16 byte expected.",
        )),
    }
}

/// Converts 1 byte value into u8
/// Errors if data found that is not 1 byte
pub fn slice_to_u8(data: &[u8]) -> StdResult<u8> {
    if data.len() == 1 {
        Ok(data[0])
    } else {
        Err(StdError::generic_err(
            "Corrupted data found. 1 byte expected.",
        ))
    }
}
