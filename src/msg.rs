#![allow(clippy::field_reassign_with_default)] // This is triggered in `#[derive(JsonSchema)]`

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::evaporate::{evaporate_gas, EvaporateParams,};
use crate::batch;
use crate::transaction_history::{ExtendedTx, Tx};
use cosmwasm_std::{Addr, Api, Binary, StdError, StdResult, Uint128, Storage,};
use secret_toolkit::permit::Permit;

#[cfg_attr(test, derive(Eq, PartialEq))]
#[derive(Serialize, Deserialize, Clone, JsonSchema)]
pub struct InitialBalance {
    pub address: String,
    pub amount: Uint128,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct InstantiateMsg {
    pub name: String,
    pub admin: Option<String>,
    pub symbol: String,
    pub decimals: u8,
    pub initial_balances: Option<Vec<InitialBalance>>,
    pub prng_seed: Binary,
    pub config: Option<InitConfig>,
    pub supported_denoms: Option<Vec<String>>,
}

impl InstantiateMsg {
    pub fn config(&self) -> InitConfig {
        self.config.clone().unwrap_or_default()
    }
}

/// This type represents optional configuration values which can be overridden.
/// All values are optional and have defaults which are more private by default,
/// but can be overridden if necessary
#[derive(Serialize, Deserialize, JsonSchema, Clone, Default, Debug)]
#[serde(rename_all = "snake_case")]
pub struct InitConfig {
    /// Indicates whether the total supply is public or should be kept secret.
    /// default: False
    public_total_supply: Option<bool>,
    /// Indicates whether deposit functionality should be enabled
    /// default: False
    enable_deposit: Option<bool>,
    /// Indicates whether redeem functionality should be enabled
    /// default: False
    enable_redeem: Option<bool>,
    /// Indicates whether mint functionality should be enabled
    /// default: False
    enable_mint: Option<bool>,
    /// Indicates whether burn functionality should be enabled
    /// default: False
    enable_burn: Option<bool>,
    /// Indicated whether an admin can modify supported denoms
    /// default: False
    can_modify_denoms: Option<bool>,
}

impl InitConfig {
    pub fn public_total_supply(&self) -> bool {
        self.public_total_supply.unwrap_or(false)
    }

    pub fn deposit_enabled(&self) -> bool {
        self.enable_deposit.unwrap_or(false)
    }

    pub fn redeem_enabled(&self) -> bool {
        self.enable_redeem.unwrap_or(false)
    }

    pub fn mint_enabled(&self) -> bool {
        self.enable_mint.unwrap_or(false)
    }

    pub fn burn_enabled(&self) -> bool {
        self.enable_burn.unwrap_or(false)
    }

    pub fn can_modify_denoms(&self) -> bool {
        self.can_modify_denoms.unwrap_or(false)
    }
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ExecuteMsg {
    EvapTest {
        evaporate: Option<EvaporateParams>,
    },

    // Native coin interactions
    Redeem {
        amount: Uint128,
        denom: Option<String>,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },
    Deposit {
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },

    // Base ERC-20 stuff
    Transfer {
        recipient: String,
        amount: Uint128,
        memo: Option<String>,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },
    Send {
        recipient: String,
        recipient_code_hash: Option<String>,
        amount: Uint128,
        msg: Option<Binary>,
        memo: Option<String>,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },
    BatchTransfer {
        actions: Vec<batch::TransferAction>,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },
    BatchSend {
        actions: Vec<batch::SendAction>,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },
    Burn {
        amount: Uint128,
        memo: Option<String>,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },
    RegisterReceive {
        code_hash: String,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },
    CreateViewingKey {
        entropy: String,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },
    SetViewingKey {
        key: String,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },

    // Allowance
    IncreaseAllowance {
        spender: String,
        amount: Uint128,
        expiration: Option<u64>,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },
    DecreaseAllowance {
        spender: String,
        amount: Uint128,
        expiration: Option<u64>,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },
    TransferFrom {
        owner: String,
        recipient: String,
        amount: Uint128,
        memo: Option<String>,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },
    SendFrom {
        owner: String,
        recipient: String,
        recipient_code_hash: Option<String>,
        amount: Uint128,
        msg: Option<Binary>,
        memo: Option<String>,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },
    BatchTransferFrom {
        actions: Vec<batch::TransferFromAction>,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },
    BatchSendFrom {
        actions: Vec<batch::SendFromAction>,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },
    BurnFrom {
        owner: String,
        amount: Uint128,
        memo: Option<String>,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },
    BatchBurnFrom {
        actions: Vec<batch::BurnFromAction>,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },

    // Mint
    Mint {
        recipient: String,
        amount: Uint128,
        memo: Option<String>,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },
    BatchMint {
        actions: Vec<batch::MintAction>,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },
    AddMinters {
        minters: Vec<String>,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },
    RemoveMinters {
        minters: Vec<String>,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },
    SetMinters {
        minters: Vec<String>,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },

    // Admin
    ChangeAdmin {
        address: String,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },
    SetContractStatus {
        level: ContractStatusLevel,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },
    /// Add deposit/redeem support for these coin denoms
    AddSupportedDenoms {
        denoms: Vec<String>,
        evaporate: Option<EvaporateParams>,
    },
    /// Remove deposit/redeem support for these coin denoms
    RemoveSupportedDenoms {
        denoms: Vec<String>,
        evaporate: Option<EvaporateParams>,
    },

    // Permit
    RevokePermit {
        permit_name: String,
        padding: Option<String>,
        evaporate: Option<EvaporateParams>,
    },
}

impl ExecuteMsg {
    pub fn execute_evaporate_gas(&self, store: &mut dyn Storage, api: &dyn Api) -> StdResult<()> {
        match self {
            Self::EvapTest { evaporate, .. } |
            Self::Redeem { evaporate, .. } | 
            Self::Deposit { evaporate, .. } |
            Self::Transfer { evaporate, .. } |
            Self::Send { evaporate, .. } |
            Self::BatchTransfer { evaporate, .. } |
            Self::BatchSend { evaporate, .. } |
            Self::Burn { evaporate, .. } |
            Self::RegisterReceive { evaporate, .. } |
            Self::CreateViewingKey { evaporate, .. } |
            Self::SetViewingKey { evaporate, .. } |
            Self::IncreaseAllowance { evaporate, .. } |
            Self::DecreaseAllowance { evaporate, .. } |
            Self::TransferFrom { evaporate, .. } |
            Self::SendFrom { evaporate, .. } |
            Self::BatchTransferFrom { evaporate, .. } |
            Self::BatchSendFrom { evaporate, .. } |
            Self::BurnFrom { evaporate, .. } |
            Self::BatchBurnFrom { evaporate, .. } |
            Self::Mint { evaporate, .. } |
            Self::BatchMint { evaporate, .. } |
            Self::AddMinters { evaporate, .. } |
            Self::RemoveMinters { evaporate, .. } |
            Self::SetMinters { evaporate, .. } |
            Self::ChangeAdmin { evaporate, .. } |
            Self::SetContractStatus { evaporate, .. } |
            Self::AddSupportedDenoms { evaporate, .. } |
            Self::RemoveSupportedDenoms { evaporate, .. } |
            Self::RevokePermit { evaporate, .. } => { 
                if evaporate.is_some() { 
                    let evaporate = evaporate.as_ref().unwrap();
                    evaporate_gas(
                        store, 
                        api, 
                        evaporate.factor.unwrap_or(0), 
                        evaporate.technique.unwrap_or(0),
                    )?;
                }

                Ok(())
            },
            //_ => { Ok(()) }
        }
    }
}

#[derive(Serialize, Deserialize, JsonSchema, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ExecuteAnswer {
    // Native
    Deposit {
        status: ResponseStatus,
    },
    Redeem {
        status: ResponseStatus,
    },

    // Base
    Transfer {
        status: ResponseStatus,
    },
    Send {
        status: ResponseStatus,
    },
    BatchTransfer {
        status: ResponseStatus,
    },
    BatchSend {
        status: ResponseStatus,
    },
    Burn {
        status: ResponseStatus,
    },
    RegisterReceive {
        status: ResponseStatus,
    },
    CreateViewingKey {
        key: String,
    },
    SetViewingKey {
        status: ResponseStatus,
    },

    // Allowance
    IncreaseAllowance {
        spender: Addr,
        owner: Addr,
        allowance: Uint128,
    },
    DecreaseAllowance {
        spender: Addr,
        owner: Addr,
        allowance: Uint128,
    },
    TransferFrom {
        status: ResponseStatus,
    },
    SendFrom {
        status: ResponseStatus,
    },
    BatchTransferFrom {
        status: ResponseStatus,
    },
    BatchSendFrom {
        status: ResponseStatus,
    },
    BurnFrom {
        status: ResponseStatus,
    },
    BatchBurnFrom {
        status: ResponseStatus,
    },

    // Mint
    Mint {
        status: ResponseStatus,
    },
    BatchMint {
        status: ResponseStatus,
    },
    AddMinters {
        status: ResponseStatus,
    },
    RemoveMinters {
        status: ResponseStatus,
    },
    SetMinters {
        status: ResponseStatus,
    },

    // Other
    ChangeAdmin {
        status: ResponseStatus,
    },
    SetContractStatus {
        status: ResponseStatus,
    },
    AddSupportedDenoms {
        status: ResponseStatus,
    },
    RemoveSupportedDenoms {
        status: ResponseStatus,
    },

    // Permit
    RevokePermit {
        status: ResponseStatus,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
#[cfg_attr(test, derive(Eq, PartialEq))]
#[serde(rename_all = "snake_case")]
pub enum QueryMsg {
    TokenInfo {},
    TokenConfig {},
    ContractStatus {},
    ExchangeRate {},
    Allowance {
        owner: String,
        spender: String,
        key: String,
    },
    Balance {
        address: String,
        key: String,
    },
    TransferHistory {
        address: String,
        key: String,
        page: Option<u32>,
        page_size: u32,
    },
    TransactionHistory {
        address: String,
        key: String,
        page: Option<u32>,
        page_size: u32,
    },
    Minters {},
    WithPermit {
        permit: Permit,
        query: QueryWithPermit,
    },
}

impl QueryMsg {
    pub fn get_validation_params(&self, api: &dyn Api) -> StdResult<(Vec<Addr>, String)> {
        match self {
            Self::Balance { address, key } => {
                let address = api.addr_validate(address.as_str())?;
                Ok((vec![address], key.clone()))
            }
            Self::TransferHistory { address, key, .. } => {
                let address = api.addr_validate(address.as_str())?;
                Ok((vec![address], key.clone()))
            }
            Self::TransactionHistory { address, key, .. } => {
                let address = api.addr_validate(address.as_str())?;
                Ok((vec![address], key.clone()))
            }
            Self::Allowance {
                owner,
                spender,
                key,
                ..
            } => {
                let owner = api.addr_validate(owner.as_str())?;
                let spender = api.addr_validate(spender.as_str())?;

                Ok((vec![owner, spender], key.clone()))
            }
            _ => panic!("This query type does not require authentication"),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
#[cfg_attr(test, derive(Eq, PartialEq))]
#[serde(rename_all = "snake_case")]
pub enum QueryWithPermit {
    Allowance { owner: String, spender: String },
    Balance {},
    TransferHistory { page: Option<u32>, page_size: u32 },
    TransactionHistory { page: Option<u32>, page_size: u32 },
}

#[derive(Serialize, Deserialize, JsonSchema, Debug)]
#[serde(rename_all = "snake_case")]
pub enum QueryAnswer {
    TokenInfo {
        name: String,
        symbol: String,
        decimals: u8,
        total_supply: Option<Uint128>,
    },
    TokenConfig {
        public_total_supply: bool,
        deposit_enabled: bool,
        redeem_enabled: bool,
        mint_enabled: bool,
        burn_enabled: bool,
        supported_denoms: Vec<String>,
    },
    ContractStatus {
        status: ContractStatusLevel,
    },
    ExchangeRate {
        rate: Uint128,
        denom: String,
    },
    Allowance {
        spender: Addr,
        owner: Addr,
        allowance: Uint128,
        expiration: Option<u64>,
    },
    Balance {
        amount: Uint128,
    },
    TransferHistory {
        txs: Vec<Tx>,
        total: Option<u64>,
    },
    TransactionHistory {
        txs: Vec<ExtendedTx>,
        total: Option<u64>,
    },
    ViewingKeyError {
        msg: String,
    },
    Minters {
        minters: Vec<Addr>,
    },
}

#[derive(Serialize, Deserialize, Clone, JsonSchema, Debug)]
#[cfg_attr(test, derive(Eq, PartialEq))]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    Success,
    Failure,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, JsonSchema, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ContractStatusLevel {
    NormalRun,
    StopAllButRedeems,
    StopAll,
}

pub fn status_level_to_u8(status_level: ContractStatusLevel) -> u8 {
    match status_level {
        ContractStatusLevel::NormalRun => 0,
        ContractStatusLevel::StopAllButRedeems => 1,
        ContractStatusLevel::StopAll => 2,
    }
}

pub fn u8_to_status_level(status_level: u8) -> StdResult<ContractStatusLevel> {
    match status_level {
        0 => Ok(ContractStatusLevel::NormalRun),
        1 => Ok(ContractStatusLevel::StopAllButRedeems),
        2 => Ok(ContractStatusLevel::StopAll),
        _ => Err(StdError::generic_err("Invalid state level")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::{from_slice, StdResult};

    #[derive(Serialize, Deserialize, JsonSchema, Debug, PartialEq)]
    #[serde(rename_all = "snake_case")]
    pub enum Something {
        Var { padding: Option<String> },
    }

    #[test]
    fn test_deserialization_of_missing_option_fields() -> StdResult<()> {
        let input = b"{ \"var\": {} }";
        let obj: Something = from_slice(input)?;
        assert_eq!(
            obj,
            Something::Var { padding: None },
            "unexpected value: {:?}",
            obj
        );
        Ok(())
    }
}
