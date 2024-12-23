#![allow(clippy::field_reassign_with_default)] // This is triggered in `#[derive(JsonSchema)]`

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{batch, transaction_history::Tx};
#[cfg(feature = "gas_evaporation")]
use cosmwasm_std::Uint64;
use cosmwasm_std::{Addr, Api, Binary, StdError, StdResult, Uint128, Uint64};
use secret_toolkit::{
    notification::ChannelInfoData,
    permit::{AllRevocation, AllRevokedInterval, Permit},
};

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
    /// Indicates whether an admin can modify supported denoms
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
    // Native coin interactions
    Redeem {
        amount: Uint128,
        denom: Option<String>,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },
    Deposit {
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },

    // Base ERC-20 stuff
    Transfer {
        recipient: String,
        amount: Uint128,
        memo: Option<String>,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },
    Send {
        recipient: String,
        recipient_code_hash: Option<String>,
        amount: Uint128,
        msg: Option<Binary>,
        memo: Option<String>,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },
    BatchTransfer {
        actions: Vec<batch::TransferAction>,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },
    BatchSend {
        actions: Vec<batch::SendAction>,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },
    Burn {
        amount: Uint128,
        memo: Option<String>,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },
    RegisterReceive {
        code_hash: String,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },
    CreateViewingKey {
        entropy: Option<String>,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },
    SetViewingKey {
        key: String,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },

    // Allowance
    IncreaseAllowance {
        spender: String,
        amount: Uint128,
        expiration: Option<u64>,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },
    DecreaseAllowance {
        spender: String,
        amount: Uint128,
        expiration: Option<u64>,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },
    TransferFrom {
        owner: String,
        recipient: String,
        amount: Uint128,
        memo: Option<String>,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },
    SendFrom {
        owner: String,
        recipient: String,
        recipient_code_hash: Option<String>,
        amount: Uint128,
        msg: Option<Binary>,
        memo: Option<String>,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },
    BatchTransferFrom {
        actions: Vec<batch::TransferFromAction>,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },
    BatchSendFrom {
        actions: Vec<batch::SendFromAction>,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },
    BurnFrom {
        owner: String,
        amount: Uint128,
        memo: Option<String>,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },
    BatchBurnFrom {
        actions: Vec<batch::BurnFromAction>,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },

    // Mint
    Mint {
        recipient: String,
        amount: Uint128,
        memo: Option<String>,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },
    BatchMint {
        actions: Vec<batch::MintAction>,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },
    AddMinters {
        minters: Vec<String>,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },
    RemoveMinters {
        minters: Vec<String>,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },
    SetMinters {
        minters: Vec<String>,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },

    // Admin
    ChangeAdmin {
        address: String,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },
    SetContractStatus {
        level: ContractStatusLevel,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },
    /// Add deposit/redeem support for these coin denoms
    AddSupportedDenoms {
        denoms: Vec<String>,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
    },
    /// Remove deposit/redeem support for these coin denoms
    RemoveSupportedDenoms {
        denoms: Vec<String>,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
    },
    /// Enable or disable SNIP-52 notifications
    SetNotificationStatus {
        enabled: bool,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
    },

    // Permit
    RevokePermit {
        permit_name: String,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
        padding: Option<String>,
    },

    // SNIP 24.1 Blanket Permits
    /// Revokes all permits. Client can supply a datetime for created_after, created_before, both, or neither.
    /// * created_before – makes it so any permits using a created value less than this datetime will be rejected
    /// * created_after – makes it so any permits using a created value greater than this datetime will be rejected
    /// * both created_before and created_after – makes it so any permits using a created value between these two datetimes will be rejected
    /// * neither – makes it so ANY permit will be rejected.
    ///   in this case, the contract MUST return a revocation ID of "REVOKED_ALL". this action is idempotent
    RevokeAllPermits {
        interval: AllRevokedInterval,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
    },

    /// Deletes a previously issued permit revocation.
    DeletePermitRevocation {
        revocation_id: String,
        #[cfg(feature = "gas_evaporation")]
        gas_target: Option<Uint64>,
    },
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
    SetNotificationStatus {
        status: ResponseStatus,
    },

    // Permit
    RevokePermit {
        status: ResponseStatus,
    },

    // SNIP 24.1 - Blanket Permits
    RevokeAllPermits {
        status: ResponseStatus,
        revocation_id: Option<String>,
    },

    DeletePermitRevocation {
        status: ResponseStatus,
    },
}

#[cfg(feature = "gas_evaporation")]
pub trait Evaporator {
    fn evaporate_to_target(&self, api: &dyn Api) -> StdResult<u64>;
}

#[cfg(feature = "gas_evaporation")]
impl Evaporator for ExecuteMsg {
    fn evaporate_to_target(&self, api: &dyn Api) -> StdResult<u64> {
        match self {
            ExecuteMsg::Redeem { gas_target, .. }
            | ExecuteMsg::Deposit { gas_target, .. }
            | ExecuteMsg::Transfer { gas_target, .. }
            | ExecuteMsg::Send { gas_target, .. }
            | ExecuteMsg::BatchTransfer { gas_target, .. }
            | ExecuteMsg::BatchSend { gas_target, .. }
            | ExecuteMsg::Burn { gas_target, .. }
            | ExecuteMsg::RegisterReceive { gas_target, .. }
            | ExecuteMsg::CreateViewingKey { gas_target, .. }
            | ExecuteMsg::SetViewingKey { gas_target, .. }
            | ExecuteMsg::IncreaseAllowance { gas_target, .. }
            | ExecuteMsg::DecreaseAllowance { gas_target, .. }
            | ExecuteMsg::TransferFrom { gas_target, .. }
            | ExecuteMsg::SendFrom { gas_target, .. }
            | ExecuteMsg::BatchTransferFrom { gas_target, .. }
            | ExecuteMsg::BatchSendFrom { gas_target, .. }
            | ExecuteMsg::BurnFrom { gas_target, .. }
            | ExecuteMsg::BatchBurnFrom { gas_target, .. }
            | ExecuteMsg::Mint { gas_target, .. }
            | ExecuteMsg::BatchMint { gas_target, .. }
            | ExecuteMsg::AddMinters { gas_target, .. }
            | ExecuteMsg::RemoveMinters { gas_target, .. }
            | ExecuteMsg::SetMinters { gas_target, .. }
            | ExecuteMsg::ChangeAdmin { gas_target, .. }
            | ExecuteMsg::SetContractStatus { gas_target, .. }
            | ExecuteMsg::AddSupportedDenoms { gas_target, .. }
            | ExecuteMsg::RemoveSupportedDenoms { gas_target, .. }
            | ExecuteMsg::SetNotificationStatus { gas_targe, .. }
            | ExecuteMsg::RevokePermit { gas_target, .. }
            | ExecuteMsg::RevokeAllPermits { gas_target, .. }
            | ExecuteMsg::DeletePermitRevocation { gas_target, .. } => match gas_target {
                Some(gas_target) => {
                    let gas_used = api.check_gas()?;
                    if gas_used < gas_target.u64() {
                        let evaporate_amount = gas_target.u64() - gas_used;
                        api.gas_evaporate(evaporate_amount as u32)?;
                        return Ok(evaporate_amount);
                    }
                    Ok(0)
                }
                None => Ok(0),
            },
        }
    }
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
    AllowancesGiven {
        owner: String,
        key: String,
        page: Option<u32>,
        page_size: u32,
    },
    AllowancesReceived {
        spender: String,
        key: String,
        page: Option<u32>,
        page_size: u32,
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

    // SNIP-52 Private Push Notifications
    /// Public query to list all notification channels
    ListChannels {},
    /// Authenticated query allows clients to obtain the seed
    /// and schema for a specific channel.
    ChannelInfo {
        channels: Vec<String>,
        txhash: Option<String>,
        viewer: ViewerInfo,
    },

    // SNIP 24.1
    ListPermitRevocations {
        // `page` and `page_size` do nothing here because max revocations is only 10 but included
        // to satisfy the SNIP24.1 spec
        page: Option<u32>,
        page_size: Option<u32>,
        viewer: ViewerInfo,
    },

    WithPermit {
        permit: Permit,
        query: QueryWithPermit,
    },

    // for debug purposes only
    #[cfg(feature = "gas_tracking")]
    Dwb {},
}

/// the address and viewing key making an authenticated query request
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct ViewerInfo {
    /// querying address
    pub address: String,
    /// authentication key string
    pub viewing_key: String,
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
            Self::AllowancesGiven { owner, key, .. } => {
                let owner = api.addr_validate(owner.as_str())?;
                Ok((vec![owner], key.clone()))
            }
            Self::AllowancesReceived { spender, key, .. } => {
                let spender = api.addr_validate(spender.as_str())?;
                Ok((vec![spender], key.clone()))
            }
            Self::ChannelInfo { viewer, .. } => {
                let address = api.addr_validate(viewer.address.as_str())?;
                Ok((vec![address], viewer.viewing_key.clone()))
            }
            Self::ListPermitRevocations { viewer, .. } => {
                let address = api.addr_validate(viewer.address.as_str())?;
                Ok((vec![address], viewer.viewing_key.clone()))
            }
            _ => panic!("This query type does not require authentication"),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
#[cfg_attr(test, derive(Eq, PartialEq))]
#[serde(rename_all = "snake_case")]
pub enum QueryWithPermit {
    Allowance {
        owner: String,
        spender: String,
    },
    AllowancesGiven {
        owner: String,
        page: Option<u32>,
        page_size: u32,
    },
    AllowancesReceived {
        spender: String,
        page: Option<u32>,
        page_size: u32,
    },
    Balance {},
    TransferHistory {
        page: Option<u32>,
        page_size: u32,
    },
    TransactionHistory {
        page: Option<u32>,
        page_size: u32,
    },
    // SNIP-52 Private Push Notifications
    ChannelInfo {
        channels: Vec<String>,
        txhash: Option<String>,
    },
    // SNIP 24.1
    ListPermitRevocations {
        // `page` and `page_size` do nothing here because max revocations is only 10 but included
        // to satisfy the SNIP24.1 spec
        page: Option<u32>,
        page_size: Option<u32>,
    },
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
    AllowancesGiven {
        owner: Addr,
        allowances: Vec<AllowanceGivenResult>,
        count: u32,
    },
    AllowancesReceived {
        spender: Addr,
        allowances: Vec<AllowanceReceivedResult>,
        count: u32,
    },
    Balance {
        amount: Uint128,
    },
    TransactionHistory {
        txs: Vec<Tx>,
        total: Option<u64>,
    },
    ViewingKeyError {
        msg: String,
    },
    Minters {
        minters: Vec<Addr>,
    },

    // SNIP-52 Private Push Notifications
    ListChannels {
        channels: Vec<String>,
    },
    ChannelInfo {
        /// scopes validity of this response
        as_of_block: Uint64,
        /// shared secret in base64
        seed: Binary,
        channels: Vec<ChannelInfoData>,
    },

    // SNIP-24.1
    ListPermitRevocations {
        revocations: Vec<AllRevocation>,
    },

    #[cfg(feature = "gas_tracking")]
    Dwb {
        dwb: String,
    },
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct AllowanceGivenResult {
    pub spender: Addr,
    pub allowance: Uint128,
    pub expiration: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct AllowanceReceivedResult {
    pub owner: Addr,
    pub allowance: Uint128,
    pub expiration: Option<u64>,
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
