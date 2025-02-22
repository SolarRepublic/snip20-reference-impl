use std::any::type_name;
use std::convert::TryFrom;

use cosmwasm_std::{
    Api, CanonicalAddr, Coin, Addr, StdError, StdResult, Storage,
};
use cosmwasm_storage::{PrefixedStorage, ReadonlyPrefixedStorage};
use secret_toolkit::storage::Item;

use crate::{legacy_append_store::AppendStore, legacy_viewing_key}; //TypedStore, TypedStoreMut};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

//use crate::viewing_key::ViewingKey;
use serde::de::DeserializeOwned;

const PREFIX_TXS: &[u8] = b"transactions";
const PREFIX_TRANSFERS: &[u8] = b"transfers";

pub const KEY_CONSTANTS: &[u8] = b"constants";
pub const KEY_TOTAL_SUPPLY: &[u8] = b"total_supply";
pub const KEY_CONTRACT_STATUS: &[u8] = b"contract_status";
pub const KEY_MINTERS: &[u8] = b"minters";
pub const KEY_VK_SEED: &[u8] = b"vk::seed";

pub static VKSEED: Item<Vec<u8>> = Item::new(KEY_VK_SEED);

pub const PREFIX_CONFIG: &[u8] = b"config";
pub const PREFIX_BALANCES: &[u8] = b"balances";
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    // The block time and block height are optional so that the JSON schema
    // reflects that some SNIP-20 contracts may not include this info.
    pub block_time: Option<u64>,
    pub block_height: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TxAction {
    Transfer {
        from: Addr,
        sender: Addr,
        recipient: Addr,
    },
    Mint {
        minter: Addr,
        recipient: Addr,
    },
    Burn {
        burner: Addr,
        owner: Addr,
    },
    Deposit {},
    Redeem {},
}

// Note that id is a globally incrementing counter.
// Since it's 64 bits long, even at 50 tx/s it would take
// over 11 billion years for it to rollback. I'm pretty sure
// we'll have bigger issues by then.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct RichTx {
    pub id: u64,
    pub action: TxAction,
    pub coins: Coin,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    pub block_time: u64,
    pub block_height: u64,
}

// Stored types:

/// This type is the stored version of the legacy transfers
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
struct StoredLegacyTransfer {
    id: u64,
    from: CanonicalAddr,
    sender: CanonicalAddr,
    receiver: CanonicalAddr,
    coins: Coin,
    memo: Option<String>,
    block_time: u64,
    block_height: u64,
}

impl StoredLegacyTransfer {
    pub fn into_humanized(self, api: &dyn Api) -> StdResult<Tx> {
        let tx = Tx {
            id: self.id,
            from: api.addr_humanize(&self.from)?,
            sender: api.addr_humanize(&self.sender)?,
            receiver: api.addr_humanize(&self.receiver)?,
            coins: self.coins,
            memo: self.memo,
            block_time: Some(self.block_time),
            block_height: Some(self.block_height),
        };
        Ok(tx)
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
enum TxCode {
    Transfer = 0,
    Mint = 1,
    Burn = 2,
    Deposit = 3,
    Redeem = 4,
}

impl TxCode {

    fn from_u8(n: u8) -> StdResult<Self> {
        use TxCode::*;
        match n {
            0 => Ok(Transfer),
            1 => Ok(Mint),
            2 => Ok(Burn),
            3 => Ok(Deposit),
            4 => Ok(Redeem),
            other => Err(StdError::generic_err(format!(
                "Unexpected Tx code in transaction history: {} Storage is corrupted.",
                other
            ))),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
struct StoredTxAction {
    tx_type: u8,
    address1: Option<CanonicalAddr>,
    address2: Option<CanonicalAddr>,
    address3: Option<CanonicalAddr>,
}

impl StoredTxAction {

    fn into_humanized(self, api: &dyn Api) -> StdResult<TxAction> {
        let transfer_addr_err = || {
            StdError::generic_err(
                "Missing address in stored Transfer transaction. Storage is corrupt",
            )
        };
        let mint_addr_err = || {
            StdError::generic_err("Missing address in stored Mint transaction. Storage is corrupt")
        };
        let burn_addr_err = || {
            StdError::generic_err("Missing address in stored Burn transaction. Storage is corrupt")
        };

        // In all of these, we ignore fields that we don't expect to find populated
        let action = match TxCode::from_u8(self.tx_type)? {
            TxCode::Transfer => {
                let from = self.address1.ok_or_else(transfer_addr_err)?;
                let sender = self.address2.ok_or_else(transfer_addr_err)?;
                let recipient = self.address3.ok_or_else(transfer_addr_err)?;
                let from = api.addr_humanize(&from)?;
                let sender = api.addr_humanize(&sender)?;
                let recipient = api.addr_humanize(&recipient)?;
                TxAction::Transfer {
                    from,
                    sender,
                    recipient,
                }
            }
            TxCode::Mint => {
                let minter = self.address1.ok_or_else(mint_addr_err)?;
                let recipient = self.address2.ok_or_else(mint_addr_err)?;
                let minter = api.addr_humanize(&minter)?;
                let recipient = api.addr_humanize(&recipient)?;
                TxAction::Mint { minter, recipient }
            }
            TxCode::Burn => {
                let burner = self.address1.ok_or_else(burn_addr_err)?;
                let owner = self.address2.ok_or_else(burn_addr_err)?;
                let burner = api.addr_humanize(&burner)?;
                let owner = api.addr_humanize(&owner)?;
                TxAction::Burn { burner, owner }
            }
            TxCode::Deposit => TxAction::Deposit {},
            TxCode::Redeem => TxAction::Redeem {},
        };

        Ok(action)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
struct StoredRichTx {
    id: u64,
    action: StoredTxAction,
    coins: Coin,
    memo: Option<String>,
    block_time: u64,
    block_height: u64,
}

impl StoredRichTx {
    fn into_humanized(self, api: &dyn Api) -> StdResult<RichTx> {
        Ok(RichTx {
            id: self.id,
            action: self.action.into_humanized(api)?,
            coins: self.coins,
            memo: self.memo,
            block_time: self.block_time,
            block_height: self.block_height,
        })
    }
}

pub fn get_old_txs(
    api: &dyn Api,
    storage: &dyn Storage,
    for_address: &CanonicalAddr,
    page: u32,
    page_size: u32,
) -> StdResult<(Vec<RichTx>, u64)> {
    let store = ReadonlyPrefixedStorage::multilevel(storage, &[PREFIX_TXS, for_address.as_slice()]);

    // Try to access the storage of txs for the account.
    // If it doesn't exist yet, return an empty list of transfers.
    let store = AppendStore::<StoredRichTx, _, _>::attach(&store);
    let store = if let Some(result) = store {
        result?
    } else {
        return Ok((vec![], 0));
    };

    // Take `page_size` txs starting from the latest tx, potentially skipping `page * page_size`
    // txs from the start.
    let tx_iter = store
        .iter()
        .rev()
        .skip((page * page_size) as _)
        .take(page_size as _);

    // The `and_then` here flattens the `StdResult<StdResult<RichTx>>` to an `StdResult<RichTx>`
    let txs: StdResult<Vec<RichTx>> = tx_iter
        .map(|tx| tx.map(|tx| tx.into_humanized(api)).and_then(|x| x))
        .collect();
    txs.map(|txs| (txs, store.len() as u64))
}

pub fn get_old_transfers(
    api: &dyn Api,
    storage: &dyn Storage,
    for_address: &CanonicalAddr,
    page: u32,
    page_size: u32,
) -> StdResult<(Vec<Tx>, u64)> {
    let store =
        ReadonlyPrefixedStorage::multilevel(storage, &[PREFIX_TRANSFERS, for_address.as_slice()]);

    // Try to access the storage of transfers for the account.
    // If it doesn't exist yet, return an empty list of transfers.
    let store = AppendStore::<StoredLegacyTransfer, _, _>::attach(&store);
    let store = if let Some(result) = store {
        result?
    } else {
        return Ok((vec![], 0));
    };

    // Take `page_size` txs starting from the latest tx, potentially skipping `page * page_size`
    // txs from the start.
    let transfer_iter = store
        .iter()
        .rev()
        .skip((page * page_size) as _)
        .take(page_size as _);

    // The `and_then` here flattens the `StdResult<StdResult<RichTx>>` to an `StdResult<RichTx>`
    let transfers: StdResult<Vec<Tx>> = transfer_iter
        .map(|tx| tx.map(|tx| tx.into_humanized(api)).and_then(|x| x))
        .collect();
    transfers.map(|txs| (txs, store.len() as u64))
}

pub fn get_all_old_transfers(
    api: &dyn Api,
    storage: &dyn Storage,
    for_address: &CanonicalAddr,
) -> StdResult<Vec<Tx>> {
    let store =
        ReadonlyPrefixedStorage::multilevel(storage, &[PREFIX_TRANSFERS, for_address.as_slice()]);

    // Try to access the storage of transfers for the account.
    // If it doesn't exist yet, return an empty list of transfers.
    let store = AppendStore::<StoredLegacyTransfer, _, _>::attach(&store);
    let store = if let Some(result) = store {
        result?
    } else {
        return Ok(vec![]);
    };

    // Take `page_size` txs starting from the latest tx, potentially skipping `page * page_size`
    // txs from the start.
    let transfer_iter = store
        .iter()
        .rev();

    // The `and_then` here flattens the `StdResult<StdResult<RichTx>>` to an `StdResult<RichTx>`
    let transfers: StdResult<Vec<Tx>> = transfer_iter
        .map(|tx| tx.map(|tx| tx.into_humanized(api)).and_then(|x| x))
        .collect();
    transfers.map(|txs| txs)
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
    // is deposit enabled
    pub deposit_is_enabled: bool,
    // is redeem enabled
    pub redeem_is_enabled: bool,
    // is mint enabled
    pub mint_is_enabled: bool,
    // is burn enabled
    pub burn_is_enabled: bool,
    // the address of this contract, used to validate query permits
    pub contract_address: Addr,
    // coin denoms that are supported for deposit/redeem
    pub supported_denoms: Vec<String>,
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
