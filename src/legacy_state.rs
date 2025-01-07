use cosmwasm_std::{
    Api, CanonicalAddr, Coin, Addr, StdError, StdResult, Storage, Uint128,
};
use secret_toolkit::storage::{AppendStore, Item};
use secret_toolkit_crypto::SHA256_HASH_SIZE;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const PREFIX_TXS: &[u8] = b"transactions";
const PREFIX_TRANSFERS: &[u8] = b"transfers";

pub const KEY_PRNG: &[u8] = b"prng";

pub const PREFIX_BALANCES: &[u8] = b"balances";

pub static PRNG: Item<[u8; SHA256_HASH_SIZE]> = Item::new(KEY_PRNG);

pub struct PrngStore {}
impl PrngStore {
    pub fn load(store: &dyn Storage) -> StdResult<[u8; SHA256_HASH_SIZE]> {
        PRNG.load(store).map_err(|_err| StdError::generic_err(""))
    }
}

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

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Eq, PartialEq)]
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
    Decoy {
        address: Addr,
    },
}

// Note that id is a globally incrementing counter.
// Since it's 64 bits long, even at 50 tx/s it would take
// over 11 billion years for it to rollback. I'm pretty sure
// we'll have bigger issues by then.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct ExtendedTx {
    pub id: u64,
    pub action: TxAction,
    pub coins: Coin,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    pub block_time: u64,
    pub block_height: u64,
}

// Stored types:

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct StoredCoin {
    pub denom: String,
    pub amount: u128,
}

impl From<Coin> for StoredCoin {
    fn from(value: Coin) -> Self {
        Self {
            denom: value.denom,
            amount: value.amount.u128(),
        }
    }
}

impl From<StoredCoin> for Coin {
    fn from(value: StoredCoin) -> Self {
        Self {
            denom: value.denom,
            amount: Uint128::new(value.amount),
        }
    }
}

/// This type is the stored version of the legacy transfers
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct StoredLegacyTransfer {
    id: u64,
    from: Addr,
    sender: Addr,
    receiver: Addr,
    coins: StoredCoin,
    memo: Option<String>,
    block_time: u64,
    block_height: u64,
}
static TRANSFERS: AppendStore<StoredLegacyTransfer> = AppendStore::new(PREFIX_TRANSFERS);

impl StoredLegacyTransfer {
    pub fn into_humanized(self) -> StdResult<Tx> {
        let tx = Tx {
            id: self.id,
            from: self.from,
            sender: self.sender,
            receiver: self.receiver,
            coins: self.coins.into(),
            memo: self.memo,
            block_time: Some(self.block_time),
            block_height: Some(self.block_height),
        };
        Ok(tx)
    }

    pub fn get_transfers(
        storage: &dyn Storage,
        for_address: Addr,
        page: u32,
        page_size: u32,
        should_filter_decoys: bool,
    ) -> StdResult<(Vec<Tx>, u64)> {
        let current_addr_store = TRANSFERS.add_suffix(for_address.as_bytes());
        let len = current_addr_store.get_len(storage)? as u64;
        // Take `page_size` txs starting from the latest tx, potentially skipping `page * page_size`
        // txs from the start.
        let transfer_iter = current_addr_store
            .iter(storage)?
            .rev()
            .skip((page * page_size) as _)
            .take(page_size as _);

        // The `and_then` here flattens the `StdResult<StdResult<ExtendedTx>>` to an `StdResult<ExtendedTx>`
        let transfers: StdResult<Vec<Tx>> = if should_filter_decoys {
            transfer_iter
                .filter(|transfer| match transfer {
                    Err(_) => true,
                    Ok(t) => t.block_height != 0,
                })
                .map(|tx| tx.map(|tx| tx.into_humanized()).and_then(|x| x))
                .collect()
        } else {
            transfer_iter
                .map(|tx| tx.map(|tx| tx.into_humanized()).and_then(|x| x))
                .collect()
        };

        transfers.map(|txs| (txs, len))
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
    Decoy = 255,
}

impl TxCode {
    fn to_u8(self) -> u8 {
        self as u8
    }

    fn from_u8(n: u8) -> StdResult<Self> {
        use TxCode::*;
        match n {
            0 => Ok(Transfer),
            1 => Ok(Mint),
            2 => Ok(Burn),
            3 => Ok(Deposit),
            4 => Ok(Redeem),
            255 => Ok(Decoy),
            other => Err(StdError::generic_err(format!(
                "Unexpected Tx code in transaction history: {other} Storage is corrupted.",
            ))),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
struct StoredTxAction {
    tx_type: u8,
    address1: Option<Addr>,
    address2: Option<Addr>,
    address3: Option<Addr>,
}

impl StoredTxAction {
    fn into_tx_action(self) -> StdResult<TxAction> {
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
        let decoy_addr_err = || {
            StdError::generic_err("Missing address in stored decoy transaction. Storage is corrupt")
        };

        // In all of these, we ignore fields that we don't expect to find populated
        let action = match TxCode::from_u8(self.tx_type)? {
            TxCode::Transfer => {
                let from = self.address1.ok_or_else(transfer_addr_err)?;
                let sender = self.address2.ok_or_else(transfer_addr_err)?;
                let recipient = self.address3.ok_or_else(transfer_addr_err)?;
                TxAction::Transfer {
                    from,
                    sender,
                    recipient,
                }
            }
            TxCode::Mint => {
                let minter = self.address1.ok_or_else(mint_addr_err)?;
                let recipient = self.address2.ok_or_else(mint_addr_err)?;
                TxAction::Mint { minter, recipient }
            }
            TxCode::Burn => {
                let burner = self.address1.ok_or_else(burn_addr_err)?;
                let owner = self.address2.ok_or_else(burn_addr_err)?;
                TxAction::Burn { burner, owner }
            }
            TxCode::Deposit => TxAction::Deposit {},
            TxCode::Redeem => TxAction::Redeem {},
            TxCode::Decoy => {
                let address = self.address1.ok_or_else(decoy_addr_err)?;
                TxAction::Decoy { address }
            }
        };

        Ok(action)
    }
}

static TRANSACTIONS: AppendStore<StoredExtendedTx> = AppendStore::new(PREFIX_TXS);

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct StoredExtendedTx {
    id: u64,
    action: StoredTxAction,
    coins: StoredCoin,
    memo: Option<String>,
    block_time: u64,
    block_height: u64,
}

impl StoredExtendedTx {
    fn into_humanized(self) -> StdResult<ExtendedTx> {
        Ok(ExtendedTx {
            id: self.id,
            action: self.action.into_tx_action()?,
            coins: self.coins.into(),
            memo: self.memo,
            block_time: self.block_time,
            block_height: self.block_height,
        })
    }

    pub fn get_txs(
        storage: &dyn Storage,
        for_address: Addr,
        page: u32,
        page_size: u32,
        should_filter_decoys: bool,
    ) -> StdResult<(Vec<ExtendedTx>, u64)> {
        let current_addr_store = TRANSACTIONS.add_suffix(for_address.as_bytes());
        let len = current_addr_store.get_len(storage)? as u64;

        // Take `page_size` txs starting from the latest tx, potentially skipping `page * page_size`
        // txs from the start.
        let tx_iter = current_addr_store
            .iter(storage)?
            .rev()
            .skip((page * page_size) as _)
            .take(page_size as _);

        // The `and_then` here flattens the `StdResult<StdResult<ExtendedTx>>` to an `StdResult<ExtendedTx>`
        let txs: StdResult<Vec<ExtendedTx>> = if should_filter_decoys {
            tx_iter
                .filter(|tx| match tx {
                    Err(_) => true,
                    Ok(t) => t.action.tx_type != TxCode::Decoy.to_u8(),
                })
                .map(|tx| tx.map(|tx| tx.into_humanized()).and_then(|x| x))
                .collect()
        } else {
            tx_iter
                .map(|tx| tx.map(|tx| tx.into_humanized()).and_then(|x| x))
                .collect()
        };

        txs.map(|txs| (txs, len))
    }
}

pub fn get_all_old_transfers(
    storage: &dyn Storage,
    for_address: &Addr,
) -> StdResult<(Vec<Tx>, u64)> {
    // this is only for refunding accidental transfers of snip20 to contract address during migration
    // 1000 page size is easily enough (would run out of gas before refunding that many)
    StoredLegacyTransfer::get_transfers(storage, for_address.clone(), 0, 1000, true)
}

// Balances

pub static BALANCES: Item<u128> = Item::new(PREFIX_BALANCES);
pub struct BalancesStore {}
impl BalancesStore {
    pub fn load(storage: &dyn Storage, account: &Addr) -> Option<u128> {
        let balances = BALANCES.add_suffix(account.as_str().as_bytes());
        balances.may_load(storage).unwrap_or(None)
    }
}

pub fn get_old_balance(storage: &dyn Storage, api: &dyn Api, account: &CanonicalAddr) -> Option<u128> {
    let Ok(addr) = api.addr_humanize(account) else {
        return None;
    };

    BalancesStore::load(storage, &addr)
}

pub fn clear_old_balance(storage: &mut dyn Storage, api: &dyn Api, account: &CanonicalAddr) {
    if let Ok(addr) = api.addr_humanize(account) {
        let balances_store = BALANCES.add_suffix(addr.as_str().as_bytes());
        balances_store.remove(storage);
    }
}

