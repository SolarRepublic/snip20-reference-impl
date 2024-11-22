/// This contract implements SNIP-20 standard:
/// https://github.com/SecretFoundation/SNIPs/blob/master/SNIP-20.md
use cosmwasm_std::{
    entry_point, to_binary, Addr, BankMsg, Binary, BlockInfo, CanonicalAddr, Coin, CosmosMsg,
    Deps, DepsMut, Env, MessageInfo, Response, StdError, StdResult, Storage, Uint128, Uint64,
};
#[cfg(feature = "gas_evaporation")]
use cosmwasm_std::Api;
use secret_toolkit::notification::{get_seed, notification_id, BloomParameters, ChannelInfoData, Descriptor, FlatDescriptor, Notification, NotificationData, StructDescriptor,};
use secret_toolkit::permit::{Permit, RevokedPermits, TokenPermissions};
use secret_toolkit::utils::{pad_handle_result, pad_query_result};
use secret_toolkit::viewing_key::{ViewingKey, ViewingKeyStore};
use secret_toolkit_crypto::{hkdf_sha_256, sha_256, ContractPrng};

use crate::legacy_state::VKSEED;
use crate::{batch, legacy_state, legacy_viewing_key,};

#[cfg(feature = "gas_tracking")]
use crate::dwb::log_dwb;
use crate::dwb::{DelayedWriteBuffer, DWB, TX_NODES};

use crate::btbe::{
    find_start_bundle, initialize_btbe, merge_dwb_entry, stored_balance, stored_entry, stored_tx_count
};
#[cfg(feature = "gas_tracking")]
use crate::gas_tracker::{GasTracker, LoggingExt};
#[cfg(feature = "gas_evaporation")]
use crate::msg::Evaporator;
use crate::msg::{u8_to_status_level, MigrateMsg};
use crate::msg::{
    AllowanceGivenResult, AllowanceReceivedResult, ContractStatusLevel, ExecuteAnswer, ExecuteMsg,
    InstantiateMsg, QueryAnswer, QueryMsg, QueryWithPermit, ResponseStatus::Success,
};
use crate::notifications::{
    multi_received_data, multi_spent_data, AllowanceNotificationData, ReceivedNotificationData, SpentNotificationData, 
    MULTI_RECEIVED_CHANNEL_BLOOM_K, MULTI_RECEIVED_CHANNEL_BLOOM_N, MULTI_RECEIVED_CHANNEL_ID, MULTI_RECEIVED_CHANNEL_PACKET_SIZE, MULTI_SPENT_CHANNEL_BLOOM_K, 
    MULTI_SPENT_CHANNEL_BLOOM_N, MULTI_SPENT_CHANNEL_ID, MULTI_SPENT_CHANNEL_PACKET_SIZE
};
use crate::receiver::Snip20ReceiveMsg;
use crate::state::{
    safe_add, AllowancesStore, Config, MintersStore, CHANNELS, CONFIG, CONTRACT_STATUS, INTERNAL_SECRET, TOTAL_SUPPLY
};
use crate::strings::TRANSFER_HISTORY_UNSUPPORTED_MSG;
use crate::transaction_history::{
    store_burn_action, store_deposit_action, store_mint_action, store_redeem_action, store_transfer_action, Tx
};

/// We make sure that responses from `handle` are padded to a multiple of this size.
pub const RESPONSE_BLOCK_SIZE: usize = 256;
pub const NOTIFICATION_BLOCK_SIZE: usize = 36;
pub const PREFIX_REVOKED_PERMITS: &str = "revoked_permits";

#[entry_point]
pub fn migrate(deps: DepsMut, env: Env, _msg: MigrateMsg) -> StdResult<Response> {
    // migrate old data

    // :: minters
    let minters = legacy_state::get_old_minters(deps.storage);
    MintersStore::save(deps.storage, minters)?;

    // :: total supply
    let total_supply = legacy_state::get_old_total_supply(deps.storage);
    TOTAL_SUPPLY.save(deps.storage, &total_supply)?;

    // :: contract status
    let status = legacy_state::get_old_contract_status(deps.storage);
    CONTRACT_STATUS.save(deps.storage, &u8_to_status_level(status).unwrap())?;

    // :: constants
    let constants = legacy_state::get_old_constants(deps.storage)?;
    CONFIG.save(
        deps.storage,
        &Config {
            name: constants.name,
            admin: constants.admin,
            symbol: constants.symbol,
            decimals: constants.decimals,
            total_supply_is_public: constants.total_supply_is_public,
            deposit_is_enabled: true,
            redeem_is_enabled: true,
            mint_is_enabled: false,
            burn_is_enabled: false,
            contract_address: env.contract.address,
            supported_denoms: vec!["uscrt".to_string()],
            can_modify_denoms: false,
        }
    )?;

    // :: do not migrate receivers, use old storage

    // :: do not migrate viewing keys, use old storage

    // set up dwbs

    // initialize the bitwise-trie of bucketed entries
    initialize_btbe(deps.storage)?;

    // initialize the delay write buffer
    DWB.save(deps.storage, &DelayedWriteBuffer::new()?)?;

    let rng_seed = env.block.random.as_ref().unwrap();

    // use entropy and env.random to create an internal secret for the contract
    let entropy = constants.prng_seed.as_slice();
    let entropy_len = 16 + entropy.len();
    let mut rng_entropy = Vec::with_capacity(entropy_len);
    rng_entropy.extend_from_slice(&env.block.height.to_be_bytes());
    rng_entropy.extend_from_slice(&env.block.time.seconds().to_be_bytes());
    rng_entropy.extend_from_slice(entropy);

    // Create INTERNAL_SECRET
    let salt = Some(sha_256(&rng_entropy).to_vec());
    let internal_secret = hkdf_sha_256(
        &salt,
        rng_seed.0.as_slice(),
        "contract_internal_secret".as_bytes(),
        32,
    )?;
    INTERNAL_SECRET.save(deps.storage, &internal_secret)?;

    // Hard-coded channels
    let channels: Vec<String> = vec![
        ReceivedNotificationData::CHANNEL_ID.to_string(),
        SpentNotificationData::CHANNEL_ID.to_string(),
        AllowanceNotificationData::CHANNEL_ID.to_string(),
        MULTI_RECEIVED_CHANNEL_ID.to_string(),
        MULTI_SPENT_CHANNEL_ID.to_string(),
    ];

    for channel in channels {
        CHANNELS.insert(deps.storage, &channel)?;
    }

    let vk_seed = hkdf_sha_256(
        &salt,
        rng_seed.0.as_slice(),
        "contract_viewing_key".as_bytes(),
        32,
    )?;
    VKSEED.save(deps.storage, &vk_seed)?;

    Ok(Response::default())
}

#[entry_point]
pub fn instantiate(
    _deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
    _msg: InstantiateMsg,
) -> StdResult<Response> {
    Err(StdError::generic_err("This contract can only be instantiated through `migrate` from the sscrt contract"))
}

#[entry_point]
pub fn execute(deps: DepsMut, env: Env, info: MessageInfo, msg: ExecuteMsg) -> StdResult<Response> {
    let mut rng = ContractPrng::from_env(&env);

    let contract_status = CONTRACT_STATUS.load(deps.storage)?;

    #[cfg(feature = "gas_evaporation")]
    let api = deps.api;
    match contract_status {
        ContractStatusLevel::StopAll | ContractStatusLevel::StopAllButRedeems => {
            let response = match msg {
                ExecuteMsg::SetContractStatus { level, .. } => {
                    set_contract_status(deps, info, level)
                }
                ExecuteMsg::Redeem { amount, denom, .. }
                    if contract_status == ContractStatusLevel::StopAllButRedeems =>
                {
                    try_redeem(deps, env, info, amount, denom)
                }
                _ => Err(StdError::generic_err(
                    "This contract is stopped and this action is not allowed",
                )),
            };
            return pad_handle_result(response, RESPONSE_BLOCK_SIZE);
        }
        ContractStatusLevel::NormalRun => {} // If it's a normal run just continue
    }

    let response = match msg.clone() {
        // Native
        ExecuteMsg::Deposit { .. } => try_deposit(deps, env, info, &mut rng),
        ExecuteMsg::Redeem { amount, denom, .. } => try_redeem(deps, env, info, amount, denom),

        // Base
        ExecuteMsg::Transfer {
            recipient,
            amount,
            memo,
            ..
        } => try_transfer(deps, env, info, &mut rng, recipient, amount, memo),
        ExecuteMsg::Send {
            recipient,
            recipient_code_hash,
            amount,
            msg,
            memo,
            ..
        } => try_send(
            deps,
            env,
            info,
            &mut rng,
            recipient,
            recipient_code_hash,
            amount,
            memo,
            msg,
        ),
        ExecuteMsg::BatchTransfer { actions, .. } => {
            try_batch_transfer(deps, env, info, &mut rng, actions)
        }
        ExecuteMsg::BatchSend { actions, .. } => try_batch_send(deps, env, info, &mut rng, actions),
        ExecuteMsg::Burn { amount, memo, .. } => try_burn(deps, env, info, amount, memo),
        ExecuteMsg::RegisterReceive { code_hash, .. } => {
            try_register_receive(deps, info, code_hash)
        }
        ExecuteMsg::CreateViewingKey { entropy, .. } => try_create_key(deps, env, info, entropy, &mut rng),
        ExecuteMsg::SetViewingKey { key, .. } => try_set_key(deps, info, key),

        // Allowance
        ExecuteMsg::IncreaseAllowance {
            spender,
            amount,
            expiration,
            ..
        } => try_increase_allowance(deps, env, info, spender, amount, expiration),
        ExecuteMsg::DecreaseAllowance {
            spender,
            amount,
            expiration,
            ..
        } => try_decrease_allowance(deps, env, info, spender, amount, expiration),
        ExecuteMsg::TransferFrom {
            owner,
            recipient,
            amount,
            memo,
            ..
        } => try_transfer_from(deps, &env, info, &mut rng, owner, recipient, amount, memo),
        ExecuteMsg::SendFrom {
            owner,
            recipient,
            recipient_code_hash,
            amount,
            msg,
            memo,
            ..
        } => try_send_from(
            deps,
            env,
            &info,
            &mut rng,
            owner,
            recipient,
            recipient_code_hash,
            amount,
            memo,
            msg,
        ),
        ExecuteMsg::BatchTransferFrom { actions, .. } => {
            try_batch_transfer_from(deps, &env, info, &mut rng, actions)
        }
        ExecuteMsg::BatchSendFrom { actions, .. } => {
            try_batch_send_from(deps, env, &info, &mut rng, actions)
        }
        ExecuteMsg::BurnFrom {
            owner,
            amount,
            memo,
            ..
        } => try_burn_from(deps, &env, info, owner, amount, memo),
        ExecuteMsg::BatchBurnFrom { actions, .. } => try_batch_burn_from(deps, &env, info, actions),

        // Mint
        ExecuteMsg::Mint {
            recipient,
            amount,
            memo,
            ..
        } => try_mint(deps, env, info, &mut rng, recipient, amount, memo),
        ExecuteMsg::BatchMint { actions, .. } => try_batch_mint(deps, env, info, &mut rng, actions),

        // Other
        ExecuteMsg::ChangeAdmin { address, .. } => change_admin(deps, info, address),
        ExecuteMsg::SetContractStatus { level, .. } => set_contract_status(deps, info, level),
        ExecuteMsg::AddMinters { minters, .. } => add_minters(deps, info, minters),
        ExecuteMsg::RemoveMinters { minters, .. } => remove_minters(deps, info, minters),
        ExecuteMsg::SetMinters { minters, .. } => set_minters(deps, info, minters),
        ExecuteMsg::RevokePermit { permit_name, .. } => revoke_permit(deps, info, permit_name),
        ExecuteMsg::AddSupportedDenoms { denoms, .. } => add_supported_denoms(deps, info, denoms),
        ExecuteMsg::RemoveSupportedDenoms { denoms, .. } => {
            remove_supported_denoms(deps, info, denoms)
        },
        ExecuteMsg::MigrateLegacyAccount { .. } => migrate_legacy_account(deps, env, info),
    };

    let padded_result = pad_handle_result(response, RESPONSE_BLOCK_SIZE);

    #[cfg(feature = "gas_evaporation")]
    let evaporated = msg.evaporate_to_target(api)?;

    padded_result
}

// :: migration start
fn migrate_legacy_account(
    deps: DepsMut,
    env: Env,
    info: MessageInfo
) -> StdResult<Response> {
    let sender_raw = deps.api.addr_canonicalize(info.sender.as_str())?;
    // get the address' stored balance
    let opt_balance = stored_balance(deps.storage, &sender_raw)?;

    let old_balance;

    if opt_balance.is_some() { // check if entry is in btbe
        return Err(StdError::generic_err("Account already migrated"));
    } else {
        old_balance = legacy_state::get_old_balance(deps.storage, &sender_raw);
    }

    if old_balance == None {
        return Err(StdError::generic_err("No legacy balance"));
    }

    // they might have already been a recipient so let's settle them as if they were a sender/owner in a transfer

    // load delayed write buffer
    let mut dwb = DWB.load(deps.storage)?;

    // release the address from the buffer
    let (_dwb_balance, mut dwb_entry) = dwb.release_dwb_recipient(deps.storage, &sender_raw)?;
    dwb_entry.set_recipient(&sender_raw)?;
    
    #[cfg(feature = "gas_tracking")]
    let mut tracker: GasTracker = GasTracker::new(deps.api);

    merge_dwb_entry(
        deps.storage,
        &mut dwb_entry,
        None,
        #[cfg(feature = "gas_tracking")]
        &mut tracker,
        &env.block,
    )?;    

    Ok(Response::new()
        .set_data(to_binary(&ExecuteAnswer::MigrateLegacyAccount { status: Success })?)
    )
}
// :: migration end

#[entry_point]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    pad_query_result(
        match msg {
            QueryMsg::TokenInfo {} => query_token_info(deps.storage),
            QueryMsg::TokenConfig {} => query_token_config(deps.storage),
            QueryMsg::ContractStatus {} => query_contract_status(deps.storage),
            QueryMsg::ExchangeRate {} => query_exchange_rate(deps.storage),
            QueryMsg::Minters { .. } => query_minters(deps),
            QueryMsg::ListChannels {} => query_list_channels(deps),
            QueryMsg::WithPermit { permit, query } => permit_queries(deps, env, permit, query),

            #[cfg(feature = "gas_tracking")]
            QueryMsg::Dwb {} => log_dwb(deps.storage),

            _ => viewing_keys_queries(deps, env, msg),
        },
        RESPONSE_BLOCK_SIZE,
    )
}

fn permit_queries(deps: Deps, env: Env, permit: Permit, query: QueryWithPermit) -> Result<Binary, StdError> {
    // Validate permit content
    let token_address = CONFIG.load(deps.storage)?.contract_address;

    let account = secret_toolkit::permit::validate(
        deps,
        PREFIX_REVOKED_PERMITS,
        &permit,
        token_address.into_string(),
        None,
    )?;

    // Permit validated! We can now execute the query.
    match query {
        QueryWithPermit::Balance {} => {
            if !permit.check_permission(&TokenPermissions::Balance) {
                return Err(StdError::generic_err(format!(
                    "No permission to query balance, got permissions {:?}",
                    permit.params.permissions
                )));
            }

            query_balance(deps, account)
        }
        QueryWithPermit::TransferHistory { .. } => {
            return Err(StdError::generic_err(TRANSFER_HISTORY_UNSUPPORTED_MSG));
        }
        QueryWithPermit::TransactionHistory { page, page_size } => {
            if !permit.check_permission(&TokenPermissions::History) {
                return Err(StdError::generic_err(format!(
                    "No permission to query history, got permissions {:?}",
                    permit.params.permissions
                )));
            }

            query_transactions(deps, account, page.unwrap_or(0), page_size)
        }
        QueryWithPermit::Allowance { owner, spender } => {
            if !permit.check_permission(&TokenPermissions::Allowance) {
                return Err(StdError::generic_err(format!(
                    "No permission to query allowance, got permissions {:?}",
                    permit.params.permissions
                )));
            }

            if account != owner && account != spender {
                return Err(StdError::generic_err(format!(
                    "Cannot query allowance. Requires permit for either owner {:?} or spender {:?}, got permit for {:?}",
                    owner.as_str(), spender.as_str(), account.as_str()
                )));
            }

            query_allowance(deps, owner, spender)
        }
        QueryWithPermit::AllowancesGiven {
            owner,
            page,
            page_size,
        } => {
            if account != owner {
                return Err(StdError::generic_err(
                    "Cannot query allowance. Requires permit for owner",
                ));
            }

            // we really should add a check_permission(s) function.. an owner permit should
            // just give you permissions to do everything
            if !permit.check_permission(&TokenPermissions::Allowance)
                && !permit.check_permission(&TokenPermissions::Owner)
            {
                return Err(StdError::generic_err(format!(
                    "No permission to query all allowances, got permissions {:?}",
                    permit.params.permissions
                )));
            }
            query_allowances_given(deps, account, page.unwrap_or(0), page_size)
        }
        QueryWithPermit::AllowancesReceived {
            spender,
            page,
            page_size,
        } => {
            if account != spender {
                return Err(StdError::generic_err(
                    "Cannot query allowance. Requires permit for spender",
                ));
            }

            if !permit.check_permission(&TokenPermissions::Allowance)
                && !permit.check_permission(&TokenPermissions::Owner)
            {
                return Err(StdError::generic_err(format!(
                    "No permission to query all allowed, got permissions {:?}",
                    permit.params.permissions
                )));
            }
            query_allowances_received(deps, account, page.unwrap_or(0), page_size)
        }
        QueryWithPermit::ChannelInfo { channels, txhash } => query_channel_info(
            deps,
            env,
            channels,
            txhash,
            deps.api.addr_canonicalize(account.as_str())?,
        ),
        QueryWithPermit::LegacyTransferHistory { page, page_size } => {
            if !permit.check_permission(&TokenPermissions::History) {
                return Err(StdError::generic_err(format!(
                    "No permission to query history, got permissions {:?}",
                    permit.params.permissions
                )));
            }

            query_legacy_transfer_history(
                deps,
                &Addr::unchecked(account), 
                page.unwrap_or(0), 
                page_size
            )
        }
    }
}

pub fn viewing_keys_queries(deps: Deps, env: Env,  msg: QueryMsg) -> StdResult<Binary> {
    let (addresses, key) = msg.get_validation_params(deps.api)?;

    for address in addresses {
        // legacy viewing key
        let canonical_addr = deps.api.addr_canonicalize(address.as_str())?;
        let expected_key = legacy_state::read_viewing_key(deps.storage, &canonical_addr);

        if expected_key.is_none() {
            // Checking the key will take significant time. We don't want to exit immediately if it isn't set
            // in a way which will allow to time the command and determine if a viewing key doesn't exist
            key.check_viewing_key(&[0u8; legacy_viewing_key::VIEWING_KEY_SIZE]);
        } else if key.check_viewing_key(expected_key.unwrap().as_slice()) {
            return match msg {
                // Base
                QueryMsg::Balance { address, .. } => query_balance(deps, address),
                QueryMsg::TransferHistory { .. } => {
                    return Err(StdError::generic_err(TRANSFER_HISTORY_UNSUPPORTED_MSG));
                }
                QueryMsg::TransactionHistory {
                    address,
                    page,
                    page_size,
                    ..
                } => query_transactions(deps, address, page.unwrap_or(0), page_size),
                QueryMsg::Allowance { owner, spender, .. } => query_allowance(deps, owner, spender),
                QueryMsg::AllowancesGiven {
                    owner,
                    page,
                    page_size,
                    ..
                } => query_allowances_given(deps, owner, page.unwrap_or(0), page_size),
                QueryMsg::AllowancesReceived {
                    spender,
                    page,
                    page_size,
                    ..
                } => query_allowances_received(deps, spender, page.unwrap_or(0), page_size),
                QueryMsg::ChannelInfo {
                    channels,
                    txhash,
                    viewer,
                } => query_channel_info(
                    deps,
                    env,
                    channels,
                    txhash,
                    deps.api.addr_canonicalize(viewer.address.as_str())?,
                ),
                QueryMsg::LegacyTransferHistory { 
                    address, 
                    page, 
                    page_size,
                    ..
                } => query_legacy_transfer_history(deps, &address, page.unwrap_or(0), page_size),
                _ => panic!("This query type does not require authentication"),
            };
        }
    }

    to_binary(&QueryAnswer::ViewingKeyError {
        msg: "Wrong viewing key for this address or viewing key not set".to_string(),
    })
}

// query functions

fn query_exchange_rate(storage: &dyn Storage) -> StdResult<Binary> {
    let constants = CONFIG.load(storage)?;

    if constants.deposit_is_enabled || constants.redeem_is_enabled {
        let rate: Uint128;
        let denom: String;
        // if token has more decimals than SCRT, you get magnitudes of SCRT per token
        if constants.decimals >= 6 {
            rate = Uint128::new(10u128.pow(constants.decimals as u32 - 6));
            denom = "SCRT".to_string();
        // if token has less decimals, you get magnitudes token for SCRT
        } else {
            rate = Uint128::new(10u128.pow(6 - constants.decimals as u32));
            denom = constants.symbol;
        }
        return to_binary(&QueryAnswer::ExchangeRate { rate, denom });
    }
    to_binary(&QueryAnswer::ExchangeRate {
        rate: Uint128::zero(),
        denom: String::new(),
    })
}

fn query_token_info(storage: &dyn Storage) -> StdResult<Binary> {
    let constants = CONFIG.load(storage)?;

    let total_supply = if constants.total_supply_is_public {
        Some(Uint128::new(TOTAL_SUPPLY.load(storage)?))
    } else {
        None
    };

    to_binary(&QueryAnswer::TokenInfo {
        name: constants.name,
        symbol: constants.symbol,
        decimals: constants.decimals,
        total_supply,
    })
}

fn query_token_config(storage: &dyn Storage) -> StdResult<Binary> {
    let constants = CONFIG.load(storage)?;

    to_binary(&QueryAnswer::TokenConfig {
        public_total_supply: constants.total_supply_is_public,
        deposit_enabled: constants.deposit_is_enabled,
        redeem_enabled: constants.redeem_is_enabled,
        mint_enabled: constants.mint_is_enabled,
        burn_enabled: constants.burn_is_enabled,
        supported_denoms: constants.supported_denoms,
    })
}

fn query_contract_status(storage: &dyn Storage) -> StdResult<Binary> {
    let contract_status = CONTRACT_STATUS.load(storage)?;

    to_binary(&QueryAnswer::ContractStatus {
        status: contract_status,
    })
}

pub fn query_transactions(
    deps: Deps,
    account: String,
    page: u32,
    page_size: u32,
) -> StdResult<Binary> {
    if page_size == 0 {
        return Err(StdError::generic_err("invalid page size"));
    }

    // Notice that if query_transactions() was called by a viewing-key call, the address of
    // 'account' has already been validated.
    // The address of 'account' should not be validated if query_transactions() was called by a
    // permit call, for compatibility with non-Secret addresses.
    let account = Addr::unchecked(account);
    let account_raw = deps.api.addr_canonicalize(account.as_str())?;

    let start = page * page_size;
    let mut end = start + page_size; // one more than end index

    // first check if there are any transactions in dwb
    let dwb = DWB.load(deps.storage)?;
    let dwb_index = dwb.recipient_match(&account_raw);
    let mut txs_in_dwb = vec![];
    let txs_in_dwb_count = dwb.entries[dwb_index].list_len()?;
    if dwb_index > 0 && txs_in_dwb_count > 0 && start < txs_in_dwb_count as u32 {
        // skip if start is after buffer entries
        let head_node_index = dwb.entries[dwb_index].head_node()?;
        if head_node_index > 0 {
            let head_node = TX_NODES
                .add_suffix(&head_node_index.to_be_bytes())
                .load(deps.storage);
            // begin testing
            if head_node.is_err() {
                return Err(StdError::generic_err("tx node load error case 1"));
            }
            let head_node = head_node?;
            // end testing
            txs_in_dwb = head_node.to_vec(deps.storage, deps.api)?;
        }
    }

    //let account_slice = account_raw.as_slice();
    let account_stored_entry = stored_entry(deps.storage, &account_raw)?;
    let settled_tx_count = stored_tx_count(deps.storage, &account_stored_entry)?;
    let total = txs_in_dwb_count as u32 + settled_tx_count as u32;
    if end > total {
        end = total;
    }

    let mut txs: Vec<Tx> = vec![];

    let txs_in_dwb_count = txs_in_dwb_count as u32;
    if start < txs_in_dwb_count && end < txs_in_dwb_count {
        // option 1, start and end are both in dwb
        //println!("OPTION 1");
        txs = txs_in_dwb[start as usize..end as usize].to_vec(); // reverse chronological
    } else if start < txs_in_dwb_count && end >= txs_in_dwb_count {
        // option 2, start is in dwb and end is in settled txs
        // in this case, we do not need to search for txs, just begin at last bundle and move backwards
        //println!("OPTION 2");
        txs = txs_in_dwb[start as usize..].to_vec(); // reverse chronological
        let mut txs_left = (end - start).saturating_sub(txs.len() as u32);
        if let Some(entry) = account_stored_entry {
            let tx_bundles_idx_len = entry.history_len()?;
            if tx_bundles_idx_len > 0 {
                let mut bundle_idx = tx_bundles_idx_len - 1;
                loop {
                    let tx_bundle = entry.get_tx_bundle_at(deps.storage, bundle_idx.clone())?;
                    let head_node = TX_NODES
                        .add_suffix(&tx_bundle.head_node.to_be_bytes())
                        .load(deps.storage);
                    // begin testing
                    if head_node.is_err() {
                        return Err(StdError::generic_err("tx node load error case 2"));
                    }
                    let head_node = head_node?;
                    // end testing
                    let list_len = tx_bundle.list_len as u32;
                    if txs_left <= list_len {
                        txs.extend_from_slice(
                            &head_node.to_vec(deps.storage, deps.api)?[0..txs_left as usize],
                        );
                        break;
                    }
                    txs.extend(head_node.to_vec(deps.storage, deps.api)?);
                    txs_left = txs_left.saturating_sub(list_len);
                    if bundle_idx > 0 {
                        bundle_idx -= 1;
                    } else {
                        break;
                    }
                }
            }
        }
    } else if start >= txs_in_dwb_count {
        // option 3, start is not in dwb
        // in this case, search for where the beginning bundle is using binary search

        // bundle tx offsets are chronological, but we need reverse chronological
        // so get the settled start index as if order is reversed
        //println!("OPTION 3");
        let settled_start = settled_tx_count
            .saturating_sub(start - txs_in_dwb_count)
            .saturating_sub(1);

        if let Some((bundle_idx, tx_bundle, start_at)) =
            find_start_bundle(deps.storage, &account_raw, settled_start)?
        {
            let mut txs_left = end - start;

            let head_node = TX_NODES
                .add_suffix(&tx_bundle.head_node.to_be_bytes())
                .load(deps.storage);
            // begin testing
            if head_node.is_err() {
                return Err(StdError::generic_err("tx node load error case 3"));
            }
            let head_node = head_node?;
            // end testing
            let list_len = tx_bundle.list_len as u32;
            if start_at + txs_left <= list_len {
                // this first bundle has all the txs we need
                txs = head_node.to_vec(deps.storage, deps.api)?
                    [start_at as usize..(start_at + txs_left) as usize]
                    .to_vec();
            } else {
                // get the rest of the txs in this bundle and then go back through history
                txs = head_node.to_vec(deps.storage, deps.api)?[start_at as usize..].to_vec();
                txs_left = txs_left.saturating_sub(list_len - start_at);

                if bundle_idx > 0 && txs_left > 0 {
                    // get the next earlier bundle
                    let mut bundle_idx = bundle_idx - 1;
                    if let Some(entry) = account_stored_entry {
                        loop {
                            let tx_bundle =
                                entry.get_tx_bundle_at(deps.storage, bundle_idx.clone())?;
                            let head_node = TX_NODES
                                .add_suffix(&tx_bundle.head_node.to_be_bytes())
                                .load(deps.storage);
                            // begin testing
                            if head_node.is_err() {
                                return Err(StdError::generic_err(format!(
                                    "entry address: {:?}\nentry balance: {:?}\nentry history len: {:?}\nbundle index: {}\ntx bundle head node: {}\ntx_bundle list len: {}\ntx bundle offset:{}\n", 
                                    entry.address(),
                                    entry.balance(),
                                    entry.history_len(),
                                    bundle_idx,
                                    tx_bundle.head_node,
                                    tx_bundle.list_len,
                                    tx_bundle.offset,
                                )));
                            }
                            let head_node = head_node?;
                            // end testing
                            let list_len = tx_bundle.list_len as u32;
                            if txs_left <= list_len {
                                txs.extend_from_slice(
                                    &head_node.to_vec(deps.storage, deps.api)?
                                        [0..txs_left as usize],
                                );
                                break;
                            }
                            txs.extend(head_node.to_vec(deps.storage, deps.api)?);
                            txs_left = txs_left.saturating_sub(list_len);
                            if bundle_idx > 0 {
                                bundle_idx -= 1;
                            } else {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    let result = QueryAnswer::TransactionHistory {
        txs,
        total: Some(total as u64),
    };
    to_binary(&result)
}

// :: migration start
pub fn query_legacy_transfer_history(
    deps: Deps,
    account: &Addr,
    page: u32,
    page_size: u32,
) -> StdResult<Binary> {
    let address = deps.api.addr_canonicalize(account.as_str()).unwrap();
    let txs = legacy_state::get_old_transfers(deps.api, deps.storage, &address, page, page_size)?;

    let result = QueryAnswer::LegacyTransferHistory { txs };
    to_binary(&result)
}
// :: migration end

pub fn query_balance(deps: Deps, account: String) -> StdResult<Binary> {
    // Notice that if query_balance() was called by a viewing key call, the address of 'account'
    // has already been validated.
    // The address of 'account' should not be validated if query_balance() was called by a permit
    // call, for compatibility with non-Secret addresses.
    let account = Addr::unchecked(account);
    let account = deps.api.addr_canonicalize(account.as_str())?;

    // :: added for sscrt migration
    let amount = stored_balance(deps.storage, &account)?;
    let mut balance;
    
    let dwb = DWB.load(deps.storage)?;
    let dwb_index = dwb.recipient_match(&account);

    if amount.is_none() && dwb_index == 0 {
        // no record of balance in dwb or btbe
        balance = legacy_state::get_old_balance(deps.storage, &account).unwrap_or_default();
    } else {
        balance = amount.unwrap_or_default();
        if dwb_index > 0 {
            balance = balance.saturating_add(dwb.entries[dwb_index].amount()? as u128);
        }
    }
    // :: migration end

    let amount = Uint128::new(balance);
    let response = QueryAnswer::Balance { amount };
    to_binary(&response)
}

fn query_minters(deps: Deps) -> StdResult<Binary> {
    let minters = MintersStore::load(deps.storage)?;

    let response = QueryAnswer::Minters { minters };
    to_binary(&response)
}

// *****************
// SNIP-52 query functions
// *****************

///
/// ListChannels query
///
///   Public query to list all notification channels.
///
fn query_list_channels(deps: Deps) -> StdResult<Binary> {
    let channels: Vec<String> = CHANNELS
        .iter(deps.storage)?
        .map(|channel| channel.unwrap())
        .collect();
    to_binary(&QueryAnswer::ListChannels { channels })
}

///
/// ChannelInfo query
///
///   Authenticated query allows clients to obtain the seed,
///   and Notification ID of an event for a specific tx_hash, for a specific channel.
///
fn query_channel_info(
    deps: Deps,
    env: Env,
    channels: Vec<String>,
    txhash: Option<String>,
    sender_raw: CanonicalAddr,
) -> StdResult<Binary> {
    let secret = INTERNAL_SECRET.load(deps.storage)?;
    let secret = secret.as_slice();
    let seed = get_seed(&sender_raw, secret)?;
    let mut channels_data = vec![];
    for channel in channels {
        let answer_id;
        if let Some(tx_hash) = &txhash {
            answer_id = Some(notification_id(&seed, &channel, tx_hash)?);
        } else {
            answer_id = None;
        }
        match channel.as_str() {
            ReceivedNotificationData::CHANNEL_ID => {
                let channel_info_data = ChannelInfoData {
                    mode: "txhash".to_string(),
                    channel,
                    answer_id,
                    parameters: None,
                    data: None,
                    next_id: None,
                    counter: None,
                    cddl: Some(ReceivedNotificationData::CDDL_SCHEMA.to_string()),
                };
                channels_data.push(channel_info_data);
            }
            SpentNotificationData::CHANNEL_ID => {
                let channel_info_data = ChannelInfoData {
                    mode: "txhash".to_string(),
                    channel,
                    answer_id,
                    parameters: None,
                    data: None,
                    next_id: None,
                    counter: None,
                    cddl: Some(SpentNotificationData::CDDL_SCHEMA.to_string()),
                };
                channels_data.push(channel_info_data);
            }
            AllowanceNotificationData::CHANNEL_ID => {
                let channel_info_data = ChannelInfoData {
                    mode: "txhash".to_string(),
                    channel,
                    answer_id,
                    parameters: None,
                    data: None,
                    next_id: None,
                    counter: None,
                    cddl: Some(AllowanceNotificationData::CDDL_SCHEMA.to_string()),
                };
                channels_data.push(channel_info_data);
            }
            MULTI_RECEIVED_CHANNEL_ID => {
                let channel_info_data = ChannelInfoData {
                    mode: "bloom".to_string(),
                    channel,
                    answer_id,
                    parameters: Some(BloomParameters {
                        m: 512,
                        k: MULTI_RECEIVED_CHANNEL_BLOOM_K,
                        h: "sha256".to_string(),
                    }),
                    data: Some(Descriptor {
                        r#type: format!("packet[{}]", MULTI_RECEIVED_CHANNEL_BLOOM_N),
                        version: "1".to_string(),
                        packet_size: MULTI_RECEIVED_CHANNEL_PACKET_SIZE,
                        data: StructDescriptor {
                            r#type: "struct".to_string(),
                            label: "transfer".to_string(),
                            members: vec![
                                FlatDescriptor {
                                    r#type: "uint128".to_string(),
                                    label: "amount".to_string(),
                                    description: Some(
                                        "The transfer amount in base denomination".to_string(),
                                    ),
                                },
                                FlatDescriptor {
                                    r#type: "bytes8".to_string(),
                                    label: "spender".to_string(),
                                    description: Some(
                                        "The last 8 bytes of the sender's canonical address"
                                            .to_string(),
                                    ),
                                },
                            ],
                        },
                    }),
                    counter: None,
                    next_id: None,
                    cddl: None,
                };
                channels_data.push(channel_info_data);
            }
            MULTI_SPENT_CHANNEL_ID => {
                let channel_info_data = ChannelInfoData {
                    mode: "bloom".to_string(),
                    channel,
                    answer_id,
                    parameters: Some(BloomParameters {
                        m: 512,
                        k: MULTI_SPENT_CHANNEL_BLOOM_K,
                        h: "sha256".to_string(),
                    }),
                    data: Some(Descriptor {
                        r#type: format!("packet[{}]", MULTI_SPENT_CHANNEL_BLOOM_N),
                        version: "1".to_string(),
                        packet_size: MULTI_SPENT_CHANNEL_PACKET_SIZE,
                        data: StructDescriptor {
                            r#type: "struct".to_string(),
                            label: "transfer".to_string(),
                            members: vec![
                                FlatDescriptor {
                                    r#type: "uint128".to_string(),
                                    label: "amount".to_string(),
                                    description: Some(
                                        "The transfer amount in base denomination".to_string(),
                                    ),
                                },
                                FlatDescriptor {
                                    r#type: "uint128".to_string(),
                                    label: "balance".to_string(),
                                    description: Some(
                                        "Spender's new balance after the transfer".to_string(),
                                    ),
                                },
                                FlatDescriptor {
                                    r#type: "bytes8".to_string(),
                                    label: "recipient".to_string(),
                                    description: Some(
                                        "The last 8 bytes of the recipient's canonical address"
                                            .to_string(),
                                    ),
                                },
                            ],
                        },
                    }),
                    counter: None,
                    next_id: None,
                    cddl: None,
                };
                channels_data.push(channel_info_data);
            }
            _ => {
                return Err(StdError::generic_err(format!(
                    "`{}` channel is undefined",
                    channel
                )));
            }
        }
    }

    //Ok(Binary(vec![]))
    //let schema = CHANNEL_SCHEMATA.get(deps.storage, &channel);

    to_binary(&QueryAnswer::ChannelInfo {
        as_of_block: Uint64::from(env.block.height),
        channels: channels_data,
        seed,
    })
}

// *****************
// End SNIP-52 query functions
// *****************

// execute functions

fn change_admin(deps: DepsMut, info: MessageInfo, address: String) -> StdResult<Response> {
    let address = deps.api.addr_validate(address.as_str())?;

    let mut constants = CONFIG.load(deps.storage)?;
    check_if_admin(&constants.admin, &info.sender)?;

    constants.admin = address;
    CONFIG.save(deps.storage, &constants)?;

    Ok(Response::new().set_data(to_binary(&ExecuteAnswer::ChangeAdmin { status: Success })?))
}

fn add_supported_denoms(
    deps: DepsMut,
    info: MessageInfo,
    denoms: Vec<String>,
) -> StdResult<Response> {
    let mut config = CONFIG.load(deps.storage)?;

    check_if_admin(&config.admin, &info.sender)?;
    if !config.can_modify_denoms {
        return Err(StdError::generic_err(
            "Cannot modify denoms for this contract",
        ));
    }

    for denom in denoms.iter() {
        if !config.supported_denoms.contains(denom) {
            config.supported_denoms.push(denom.clone());
        }
    }

    CONFIG.save(deps.storage, &config)?;

    Ok(
        Response::new().set_data(to_binary(&ExecuteAnswer::AddSupportedDenoms {
            status: Success,
        })?),
    )
}

fn remove_supported_denoms(
    deps: DepsMut,
    info: MessageInfo,
    denoms: Vec<String>,
) -> StdResult<Response> {
    let mut config = CONFIG.load(deps.storage)?;

    check_if_admin(&config.admin, &info.sender)?;
    if !config.can_modify_denoms {
        return Err(StdError::generic_err(
            "Cannot modify denoms for this contract",
        ));
    }

    for denom in denoms.iter() {
        config.supported_denoms.retain(|x| x != denom);
    }

    CONFIG.save(deps.storage, &config)?;

    Ok(
        Response::new().set_data(to_binary(&ExecuteAnswer::RemoveSupportedDenoms {
            status: Success,
        })?),
    )
}

#[allow(clippy::too_many_arguments)]
fn try_mint_impl(
    deps: &mut DepsMut,
    rng: &mut ContractPrng,
    minter: Addr,
    recipient: Addr,
    amount: Uint128,
    denom: String,
    memo: Option<String>,
    block: &cosmwasm_std::BlockInfo,
    #[cfg(feature = "gas_tracking")] tracker: &mut GasTracker,
) -> StdResult<()> {
    let raw_amount = amount.u128();
    let raw_recipient = deps.api.addr_canonicalize(recipient.as_str())?;
    let raw_minter = deps.api.addr_canonicalize(minter.as_str())?;

    perform_mint(
        deps.storage,
        rng,
        &raw_minter,
        &raw_recipient,
        raw_amount,
        denom,
        memo,
        block,
        #[cfg(feature = "gas_tracking")]
        tracker,
    )?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn try_mint(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    rng: &mut ContractPrng,
    recipient: String,
    amount: Uint128,
    memo: Option<String>,
) -> StdResult<Response> {
    let secret = INTERNAL_SECRET.load(deps.storage)?;
    let secret = secret.as_slice();

    let recipient = deps.api.addr_validate(recipient.as_str())?;

    let constants = CONFIG.load(deps.storage)?;

    if !constants.mint_is_enabled {
        return Err(StdError::generic_err(
            "Mint functionality is not enabled for this token.",
        ));
    }

    let minters = MintersStore::load(deps.storage)?;
    if !minters.contains(&info.sender) {
        return Err(StdError::generic_err(
            "Minting is allowed to minter accounts only",
        ));
    }

    let mut total_supply = TOTAL_SUPPLY.load(deps.storage)?;
    let minted_amount = safe_add(&mut total_supply, amount.u128());
    TOTAL_SUPPLY.save(deps.storage, &total_supply)?;

    #[cfg(feature = "gas_tracking")]
    let mut tracker: GasTracker = GasTracker::new(deps.api);

    // Note that even when minted_amount is equal to 0 we still want to perform the operations for logic consistency
    try_mint_impl(
        &mut deps,
        rng,
        info.sender,
        recipient.clone(),
        Uint128::new(minted_amount),
        constants.symbol,
        memo,
        &env.block,
        #[cfg(feature = "gas_tracking")]
        &mut tracker,
    )?;

    let received_notification = Notification::new(
        recipient,
        ReceivedNotificationData {
            amount: minted_amount,
            sender: None,
        },
    )
    .to_txhash_notification(deps.api, &env, secret, Some(NOTIFICATION_BLOCK_SIZE))?;

    let resp = Response::new()
        .set_data(to_binary(&ExecuteAnswer::Mint { status: Success })?)
        .add_attribute_plaintext(
            received_notification.id_plaintext(),
            received_notification.data_plaintext(),
        );

    #[cfg(feature = "gas_tracking")]
    return Ok(resp.add_gas_tracker(tracker));

    #[cfg(not(feature = "gas_tracking"))]
    Ok(resp)
}

fn try_batch_mint(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    rng: &mut ContractPrng,
    actions: Vec<batch::MintAction>,
) -> StdResult<Response> {
    let secret = INTERNAL_SECRET.load(deps.storage)?;
    let secret = secret.as_slice();

    let constants = CONFIG.load(deps.storage)?;

    if !constants.mint_is_enabled {
        return Err(StdError::generic_err(
            "Mint functionality is not enabled for this token.",
        ));
    }

    let minters = MintersStore::load(deps.storage)?;
    if !minters.contains(&info.sender) {
        return Err(StdError::generic_err(
            "Minting is allowed to minter accounts only",
        ));
    }

    let mut total_supply = TOTAL_SUPPLY.load(deps.storage)?;

    let mut notifications = vec![];
    // Quick loop to check that the total of amounts is valid
    for action in actions {
        let actual_amount = safe_add(&mut total_supply, action.amount.u128());

        let recipient = deps.api.addr_validate(action.recipient.as_str())?;

        #[cfg(feature = "gas_tracking")]
        let mut tracker: GasTracker = GasTracker::new(deps.api);

        try_mint_impl(
            &mut deps,
            rng,
            info.sender.clone(),
            recipient.clone(),
            Uint128::new(actual_amount),
            constants.symbol.clone(),
            action.memo,
            &env.block,
            #[cfg(feature = "gas_tracking")]
            &mut tracker,
        )?;
        notifications.push(Notification::new (
            recipient,
            ReceivedNotificationData {
                amount: actual_amount,
                sender: None,
            },
        ));
    }

    let tx_hash = env
        .transaction
        .clone()
        .ok_or(StdError::generic_err("no tx hash found"))?
        .hash;
    let received_data = multi_received_data(
        deps.api,
        notifications,
        &tx_hash,
        env.block.random.unwrap(),
        secret,
    )?;

    TOTAL_SUPPLY.save(deps.storage, &total_supply)?;

    Ok(Response::new()
        .set_data(to_binary(&ExecuteAnswer::BatchMint { status: Success })?)
        .add_attribute_plaintext(
            format!("snip52:#{}", MULTI_RECEIVED_CHANNEL_ID),
            Binary::from(received_data).to_base64(),
        )
    )
}

pub fn try_set_key(deps: DepsMut, info: MessageInfo, key: String) -> StdResult<Response> {
    ViewingKey::set(deps.storage, info.sender.as_str(), key.as_str());

    // legacy set key
    let vk = legacy_viewing_key::ViewingKey(key);

    let message_sender = deps.api.addr_canonicalize(info.sender.as_str())?;
    legacy_state::write_viewing_key(deps.storage, &message_sender, &vk);

    
    Ok(
        Response::new().set_data(to_binary(&ExecuteAnswer::SetViewingKey {
            status: Success,
        })?),
    )
}

pub fn try_create_key(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    entropy: Option<String>,
    rng: &mut ContractPrng,
) -> StdResult<Response> {
    let entropy = [entropy.unwrap_or_default().as_bytes(), &rng.rand_bytes()].concat();

    // legacy create key
    let vk_seed = VKSEED.load(deps.storage)?;
    let key = legacy_viewing_key::ViewingKey::new(&env, &info, &vk_seed, entropy.as_slice());

    let message_sender = deps.api.addr_canonicalize(info.sender.as_str())?;
    legacy_state::write_viewing_key(deps.storage, &message_sender, &key);

    Ok(Response::new().set_data(to_binary(&ExecuteAnswer::CreateViewingKey { key: key.0 })?))
}

fn set_contract_status(
    deps: DepsMut,
    info: MessageInfo,
    status_level: ContractStatusLevel,
) -> StdResult<Response> {
    let constants = CONFIG.load(deps.storage)?;
    check_if_admin(&constants.admin, &info.sender)?;

    CONTRACT_STATUS.save(deps.storage, &status_level)?;

    Ok(
        Response::new().set_data(to_binary(&ExecuteAnswer::SetContractStatus {
            status: Success,
        })?),
    )
}

pub fn query_allowance(deps: Deps, owner: String, spender: String) -> StdResult<Binary> {
    // Notice that if query_allowance() was called by a viewing-key call, the addresses of 'owner'
    // and 'spender' have already been validated.
    // The addresses of 'owner' and 'spender' should not be validated if query_allowance() was
    // called by a permit call, for compatibility with non-Secret addresses.
    let owner = Addr::unchecked(owner);
    let spender = Addr::unchecked(spender);

    let allowance = AllowancesStore::load(deps.storage, &owner, &spender);

    let response = QueryAnswer::Allowance {
        owner,
        spender,
        allowance: Uint128::new(allowance.amount),
        expiration: allowance.expiration,
    };
    to_binary(&response)
}

pub fn query_allowances_given(
    deps: Deps,
    owner: String,
    page: u32,
    page_size: u32,
) -> StdResult<Binary> {
    // Notice that if query_all_allowances_given() was called by a viewing-key call,
    // the address of 'owner' has already been validated.
    // The addresses of 'owner' should not be validated if query_all_allowances_given() was
    // called by a permit call, for compatibility with non-Secret addresses.
    let owner = Addr::unchecked(owner);

    let all_allowances =
        AllowancesStore::all_allowances(deps.storage, &owner, page, page_size).unwrap_or(vec![]);

    let allowances_result = all_allowances
        .into_iter()
        .map(|(spender, allowance)| AllowanceGivenResult {
            spender,
            allowance: Uint128::from(allowance.amount),
            expiration: allowance.expiration,
        })
        .collect();

    let response = QueryAnswer::AllowancesGiven {
        owner: owner.clone(),
        allowances: allowances_result,
        count: AllowancesStore::num_allowances(deps.storage, &owner),
    };
    to_binary(&response)
}

pub fn query_allowances_received(
    deps: Deps,
    spender: String,
    page: u32,
    page_size: u32,
) -> StdResult<Binary> {
    // Notice that if query_all_allowances_received() was called by a viewing-key call,
    // the address of 'spender' has already been validated.
    // The addresses of 'spender' should not be validated if query_all_allowances_received() was
    // called by a permit call, for compatibility with non-Secret addresses.
    let spender = Addr::unchecked(spender);

    let all_allowed =
        AllowancesStore::all_allowed(deps.storage, &spender, page, page_size).unwrap_or(vec![]);

    let allowances = all_allowed
        .into_iter()
        .map(|(owner, allowance)| AllowanceReceivedResult {
            owner,
            allowance: Uint128::from(allowance.amount),
            expiration: allowance.expiration,
        })
        .collect();

    let response = QueryAnswer::AllowancesReceived {
        spender: spender.clone(),
        allowances,
        count: AllowancesStore::num_allowed(deps.storage, &spender),
    };
    to_binary(&response)
}

fn try_deposit(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    rng: &mut ContractPrng,
) -> StdResult<Response> {
    let constants = CONFIG.load(deps.storage)?;

    let mut amount = Uint128::zero();

    for coin in &info.funds {
        if constants.supported_denoms.contains(&coin.denom) {
            amount += coin.amount
        } else {
            return Err(StdError::generic_err(format!(
                "Tried to deposit an unsupported coin {}",
                coin.denom
            )));
        }
    }

    if amount.is_zero() {
        return Err(StdError::generic_err("No funds were sent to be deposited"));
    }

    let mut raw_amount = amount.u128();

    if !constants.deposit_is_enabled {
        return Err(StdError::generic_err(
            "Deposit functionality is not enabled.",
        ));
    }

    let mut total_supply = TOTAL_SUPPLY.load(deps.storage)?;
    raw_amount = safe_add(&mut total_supply, raw_amount);
    TOTAL_SUPPLY.save(deps.storage, &total_supply)?;

    let sender_address = deps.api.addr_canonicalize(info.sender.as_str())?;

    #[cfg(feature = "gas_tracking")]
    let mut tracker: GasTracker = GasTracker::new(deps.api);

    perform_deposit(
        deps.storage,
        rng,
        &sender_address,
        raw_amount,
        "uscrt".to_string(),
        &env.block,
        #[cfg(feature = "gas_tracking")]
        &mut tracker,
    )?;

    let resp = Response::new().set_data(to_binary(&ExecuteAnswer::Deposit { status: Success })?);

    #[cfg(feature = "gas_tracking")]
    return Ok(resp.add_gas_tracker(tracker));

    #[cfg(not(feature = "gas_tracking"))]
    Ok(resp)
}

fn try_redeem(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    amount: Uint128,
    denom: Option<String>,
) -> StdResult<Response> {
    let constants = CONFIG.load(deps.storage)?;
    if !constants.redeem_is_enabled {
        return Err(StdError::generic_err(
            "Redeem functionality is not enabled for this token.",
        ));
    }

    // if denom is none and there is only 1 supported denom then we don't need to check anything
    let withdraw_denom = if denom.is_none() && constants.supported_denoms.len() == 1 {
        constants.supported_denoms.first().unwrap().clone()
    // if denom is specified make sure it's on the list before trying to withdraw with it
    } else if denom.is_some() && constants.supported_denoms.contains(denom.as_ref().unwrap()) {
        denom.unwrap()
    // error handling
    } else if denom.is_none() {
        return Err(StdError::generic_err(
            "Tried to redeem without specifying denom, but multiple coins are supported",
        ));
    } else {
        return Err(StdError::generic_err(
            "Tried to redeem for an unsupported coin",
        ));
    };

    let sender_address = deps.api.addr_canonicalize(info.sender.as_str())?;
    let amount_raw = amount.u128();

    let tx_id = store_redeem_action(deps.storage, amount.u128(), constants.symbol, &env.block)?;

    // load delayed write buffer
    let mut dwb = DWB.load(deps.storage)?;

    #[cfg(feature = "gas_tracking")]
    let mut tracker = GasTracker::new(deps.api);

    // settle the signer's account in buffer
    dwb.settle_sender_or_owner_account(
        deps.storage,
        &sender_address,
        tx_id,
        amount_raw,
        "redeem",
        #[cfg(feature = "gas_tracking")]
        &mut tracker,
        &env.block,
    )?;

    DWB.save(deps.storage, &dwb)?;

    let total_supply = TOTAL_SUPPLY.load(deps.storage)?;
    if let Some(total_supply) = total_supply.checked_sub(amount_raw) {
        TOTAL_SUPPLY.save(deps.storage, &total_supply)?;
    } else {
        return Err(StdError::generic_err(
            "You are trying to redeem more tokens than what is available in the total supply",
        ));
    }

    let token_reserve = deps
        .querier
        .query_balance(&env.contract.address, &withdraw_denom)?
        .amount;
    if amount > token_reserve {
        return Err(StdError::generic_err(format!(
            "You are trying to redeem for more {withdraw_denom} than the contract has in its reserve",
        )));
    }

    let withdrawal_coins: Vec<Coin> = vec![Coin {
        denom: withdraw_denom,
        amount,
    }];

    let message = CosmosMsg::Bank(BankMsg::Send {
        to_address: info.sender.clone().into_string(),
        amount: withdrawal_coins,
    });
    let data = to_binary(&ExecuteAnswer::Redeem { status: Success })?;
    let res = Response::new().add_message(message).set_data(data);
    Ok(res)
}

#[allow(clippy::too_many_arguments)]
fn try_transfer_impl(
    deps: &mut DepsMut,
    rng: &mut ContractPrng,
    sender: &Addr,
    recipient: &Addr,
    amount: Uint128,
    denom: String,
    memo: Option<String>,
    block: &cosmwasm_std::BlockInfo,
    #[cfg(feature = "gas_tracking")] tracker: &mut GasTracker,
) -> StdResult<(Notification<ReceivedNotificationData>, Notification<SpentNotificationData>)> {
    let raw_sender = deps.api.addr_canonicalize(sender.as_str())?;
    let raw_recipient = deps.api.addr_canonicalize(recipient.as_str())?;

    let sender_balance = perform_transfer(
        deps.storage,
        rng,
        &raw_sender,
        &raw_recipient,
        &raw_sender,
        amount.u128(),
        denom,
        memo,
        block,
        #[cfg(feature = "gas_tracking")]
        tracker,
    )?;
    let received_notification = Notification::new(
        recipient.clone(),
        ReceivedNotificationData {
            amount: amount.u128(),
            sender: Some(sender.clone()),
        }
    );

    let spent_notification = Notification::new (
        sender.clone(),
        SpentNotificationData {
            amount: amount.u128(),
            actions: 1,
            recipient: Some(recipient.clone()),
            balance: sender_balance,
        }
    );

    Ok((received_notification, spent_notification))
}

#[allow(clippy::too_many_arguments)]
fn try_transfer(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    rng: &mut ContractPrng,
    recipient: String,
    amount: Uint128,
    memo: Option<String>,
) -> StdResult<Response> {
    let secret = INTERNAL_SECRET.load(deps.storage)?;
    let secret = secret.as_slice();

    let recipient: Addr = deps.api.addr_validate(recipient.as_str())?;

    let symbol = CONFIG.load(deps.storage)?.symbol;

    #[cfg(feature = "gas_tracking")]
    let mut tracker: GasTracker = GasTracker::new(deps.api);

    let (received_notification, spent_notification) = try_transfer_impl(
        &mut deps,
        rng,
        &info.sender,
        &recipient,
        amount,
        symbol,
        memo,
        &env.block,
        #[cfg(feature = "gas_tracking")]
        &mut tracker,
    )?;

    #[cfg(feature = "gas_tracking")]
    let mut group1 = tracker.group("try_transfer.rest");

    let received_notification = received_notification.to_txhash_notification(
        deps.api,
        &env,
        secret,
        Some(NOTIFICATION_BLOCK_SIZE),
    )?;

    let spent_notification = spent_notification.to_txhash_notification(
        deps.api, 
        &env, 
        secret, 
        Some(NOTIFICATION_BLOCK_SIZE)
    )?;

    let resp = Response::new()
        .set_data(to_binary(&ExecuteAnswer::Transfer { status: Success })?)
        .add_attribute_plaintext(
            received_notification.id_plaintext(),
            received_notification.data_plaintext(),
        )
        .add_attribute_plaintext(
            spent_notification.id_plaintext(),
            spent_notification.data_plaintext(),
        );

    #[cfg(feature = "gas_tracking")]
    group1.log("rest");

    #[cfg(feature = "gas_tracking")]
    return Ok(resp.add_gas_tracker(tracker));

    #[cfg(not(feature = "gas_tracking"))]
    Ok(resp)
}

fn try_batch_transfer(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    rng: &mut ContractPrng,
    actions: Vec<batch::TransferAction>,
) -> StdResult<Response> {
    let num_actions = actions.len();
    if num_actions == 0 {
        return Ok(Response::new()
            .set_data(to_binary(&ExecuteAnswer::BatchTransfer { status: Success })?)
        );
    }

    let secret = INTERNAL_SECRET.load(deps.storage)?;
    let secret = secret.as_slice();

    let symbol = CONFIG.load(deps.storage)?.symbol;

    #[cfg(feature = "gas_tracking")]
    let mut tracker: GasTracker = GasTracker::new(deps.api);

    let mut notifications = vec![];
    for action in actions {
        let recipient = deps.api.addr_validate(action.recipient.as_str())?;
        let (received_notification, spent_notification) = try_transfer_impl(
            &mut deps,
            rng,
            &info.sender,
            &recipient,
            action.amount,
            symbol.clone(),
            action.memo,
            &env.block,
            #[cfg(feature = "gas_tracking")]
            &mut tracker,
        )?;
        notifications.push((received_notification, spent_notification));
    }

    let tx_hash = env
        .transaction
        .clone()
        .ok_or(StdError::generic_err("no tx hash found"))?
        .hash;
    let (received_notifications, spent_notifications): (
        Vec<Notification<ReceivedNotificationData>>,
        Vec<Notification<SpentNotificationData>>,
    ) = notifications.into_iter().unzip();
    let received_data = multi_received_data(
        deps.api,
        received_notifications,
        &tx_hash,
        env.block.random.clone().unwrap(),
        secret,
    )?;

    let total_amount_spent = spent_notifications
        .iter()
        .fold(0u128, |acc, notification| acc.saturating_add(notification.data.amount));

    let spent_notification = Notification::new (
        info.sender,
        SpentNotificationData {
            amount: total_amount_spent,
            actions: num_actions as u32,
            recipient: spent_notifications[0].data.recipient.clone(),
            balance: spent_notifications.last().unwrap().data.balance,
        }
    )
    .to_txhash_notification(deps.api, &env, secret, Some(NOTIFICATION_BLOCK_SIZE))?;

    let resp = Response::new()
        .set_data(to_binary(&ExecuteAnswer::BatchTransfer { status: Success })?)
        .add_attribute_plaintext(
            format!("snip52:#{}", MULTI_RECEIVED_CHANNEL_ID),
            Binary::from(received_data).to_base64(),
        )
        .add_attribute_plaintext(
            spent_notification.id_plaintext(),
            spent_notification.data_plaintext(),
        );

    #[cfg(feature = "gas_tracking")]
    return Ok(resp.add_gas_tracker(tracker));

    #[cfg(not(feature = "gas_tracking"))]
    Ok(resp)
}

#[allow(clippy::too_many_arguments)]
fn try_add_receiver_api_callback(
    storage: &dyn Storage,
    messages: &mut Vec<CosmosMsg>,
    recipient: Addr,
    recipient_code_hash: Option<String>,
    msg: Option<Binary>,
    sender: Addr,
    from: Addr,
    amount: Uint128,
    memo: Option<String>,
) -> StdResult<()> {
    if let Some(receiver_hash) = recipient_code_hash {
        let receiver_msg = Snip20ReceiveMsg::new(sender, from, amount, memo, msg);
        let callback_msg = receiver_msg.into_cosmos_msg(receiver_hash, recipient)?;

        messages.push(callback_msg);
        return Ok(());
    }

    //let receiver_hash = ReceiverHashStore::may_load(storage, &recipient)?;
    let receiver_hash = legacy_state::get_receiver_hash(storage, &recipient);
    if let Some(receiver_hash) = receiver_hash {
        let receiver_hash = receiver_hash?;
        let receiver_msg = Snip20ReceiveMsg::new(sender, from, amount, memo, msg);
        let callback_msg = receiver_msg.into_cosmos_msg(receiver_hash, recipient)?;

        messages.push(callback_msg);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn try_send_impl(
    deps: &mut DepsMut,
    rng: &mut ContractPrng,
    messages: &mut Vec<CosmosMsg>,
    sender: Addr,
    recipient: Addr,
    recipient_code_hash: Option<String>,
    amount: Uint128,
    denom: String,
    memo: Option<String>,
    msg: Option<Binary>,
    block: &cosmwasm_std::BlockInfo,
    #[cfg(feature = "gas_tracking")] tracker: &mut GasTracker,
) -> StdResult<(Notification<ReceivedNotificationData>, Notification<SpentNotificationData>)> {
    let (received_notification, spent_notification) = try_transfer_impl(
        deps,
        rng,
        &sender,
        &recipient,
        amount,
        denom,
        memo.clone(),
        block,
        #[cfg(feature = "gas_tracking")]
        tracker,
    )?;

    try_add_receiver_api_callback(
        deps.storage,
        messages,
        recipient,
        recipient_code_hash,
        msg,
        sender.clone(),
        sender,
        amount,
        memo,
    )?;

    Ok((received_notification, spent_notification))
}

#[allow(clippy::too_many_arguments)]
fn try_send(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    rng: &mut ContractPrng,
    recipient: String,
    recipient_code_hash: Option<String>,
    amount: Uint128,
    memo: Option<String>,
    msg: Option<Binary>,
) -> StdResult<Response> {
    let secret = INTERNAL_SECRET.load(deps.storage)?;
    let secret = secret.as_slice();

    let recipient = deps.api.addr_validate(recipient.as_str())?;

    let mut messages = vec![];
    let symbol = CONFIG.load(deps.storage)?.symbol;

    #[cfg(feature = "gas_tracking")]
    let mut tracker: GasTracker = GasTracker::new(deps.api);

    let (received_notification, spent_notification) = try_send_impl(
        &mut deps,
        rng,
        &mut messages,
        info.sender,
        recipient,
        recipient_code_hash,
        amount,
        symbol,
        memo,
        msg,
        &env.block,
        #[cfg(feature = "gas_tracking")]
        &mut tracker,
    )?;

    let received_notification = received_notification.to_txhash_notification(
        deps.api,
        &env,
        secret,
        Some(NOTIFICATION_BLOCK_SIZE)
    )?;
    let spent_notification =
        spent_notification.to_txhash_notification(deps.api, &env, secret, Some(NOTIFICATION_BLOCK_SIZE))?;

    let resp = Response::new()
        .add_messages(messages)
        .set_data(to_binary(&ExecuteAnswer::Send { status: Success })?)
        .add_attribute_plaintext(
            received_notification.id_plaintext(),
            received_notification.data_plaintext(),
        )
        .add_attribute_plaintext(
            spent_notification.id_plaintext(),
            spent_notification.data_plaintext(),
        );

    #[cfg(feature = "gas_tracking")]
    return Ok(resp.add_gas_tracker(tracker));

    #[cfg(not(feature = "gas_tracking"))]
    Ok(resp)
}

fn try_batch_send(
    mut deps: DepsMut,
    env: Env,
    info: MessageInfo,
    rng: &mut ContractPrng,
    actions: Vec<batch::SendAction>,
) -> StdResult<Response> {
    let num_actions = actions.len();
    if num_actions == 0 {
        return Ok(Response::new()
            .set_data(to_binary(&ExecuteAnswer::BatchSend { status: Success })?)
        );
    }

    let secret = INTERNAL_SECRET.load(deps.storage)?;
    let secret = secret.as_slice();

    let mut messages = vec![];

    let mut notifications = vec![];
    let num_actions: usize = actions.len();

    let symbol = CONFIG.load(deps.storage)?.symbol;

    #[cfg(feature = "gas_tracking")]
    let mut tracker: GasTracker = GasTracker::new(deps.api);

    for action in actions {
        let recipient = deps.api.addr_validate(action.recipient.as_str())?;
        let (received_notification, spent_notification) = try_send_impl(
            &mut deps,
            rng,
            &mut messages,
            info.sender.clone(),
            recipient,
            action.recipient_code_hash,
            action.amount,
            symbol.clone(),
            action.memo,
            action.msg,
            &env.block,
            #[cfg(feature = "gas_tracking")]
            &mut tracker,
        )?;
        notifications.push((received_notification, spent_notification));
    }

    let tx_hash = env
        .transaction
        .clone()
        .ok_or(StdError::generic_err("no tx hash found"))?
        .hash;

    let (received_notifications, spent_notifications): (
        Vec<Notification<ReceivedNotificationData>>,
        Vec<Notification<SpentNotificationData>>,
    ) = notifications.into_iter().unzip();
    let received_data = multi_received_data(
        deps.api,
        received_notifications,
        &tx_hash,
        env.block.random.clone().unwrap(),
        secret,
    )?;

    let total_amount_spent = spent_notifications
        .iter()
        .fold(0u128, |acc, notification| acc + notification.data.amount);

    let spent_notification = Notification::new (
        info.sender,
        SpentNotificationData {
            amount: total_amount_spent,
            actions: num_actions as u32,
            recipient: spent_notifications[0].data.recipient.clone(),
            balance: spent_notifications.last().unwrap().data.balance,
        }
    )
    .to_txhash_notification(deps.api, &env, secret, Some(NOTIFICATION_BLOCK_SIZE))?;

    Ok(Response::new()
        .add_messages(messages)
        .set_data(to_binary(&ExecuteAnswer::BatchSend { status: Success })?)
        .add_attribute_plaintext(
            format!("snip52:#{}", MULTI_RECEIVED_CHANNEL_ID),
            Binary::from(received_data).to_base64(),
        )
        .add_attribute_plaintext(
            spent_notification.id_plaintext(),
            spent_notification.data_plaintext(),
        )
    )
}

fn try_register_receive(
    deps: DepsMut,
    info: MessageInfo,
    code_hash: String,
) -> StdResult<Response> {
    //ReceiverHashStore::save(deps.storage, &info.sender, code_hash)?;
    legacy_state::set_receiver_hash(deps.storage, &info.sender, code_hash);

    let data = to_binary(&ExecuteAnswer::RegisterReceive { status: Success })?;
    Ok(Response::new()
        .add_attribute("register_status", "success")
        .set_data(data))
}

fn insufficient_allowance(allowance: u128, required: u128) -> StdError {
    StdError::generic_err(format!(
        "insufficient allowance: allowance={allowance}, required={required}",
    ))
}

fn use_allowance(
    storage: &mut dyn Storage,
    env: &Env,
    owner: &Addr,
    spender: &Addr,
    amount: u128,
) -> StdResult<()> {
    let mut allowance = AllowancesStore::load(storage, owner, spender);

    if allowance.is_expired_at(&env.block) || allowance.amount == 0 {
        return Err(insufficient_allowance(0, amount));
    }
    if let Some(new_allowance) = allowance.amount.checked_sub(amount) {
        allowance.amount = new_allowance;
    } else {
        return Err(insufficient_allowance(allowance.amount, amount));
    }

    AllowancesStore::save(storage, owner, spender, &allowance)?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn try_transfer_from_impl(
    deps: &mut DepsMut,
    rng: &mut ContractPrng,
    env: &Env,
    spender: &Addr,
    owner: &Addr,
    recipient: &Addr,
    amount: Uint128,
    denom: String,
    memo: Option<String>,
) -> StdResult<(Notification<ReceivedNotificationData>, Notification<SpentNotificationData>)> {
    let raw_amount = amount.u128();
    let raw_spender = deps.api.addr_canonicalize(spender.as_str())?;
    let raw_owner = deps.api.addr_canonicalize(owner.as_str())?;
    let raw_recipient = deps.api.addr_canonicalize(recipient.as_str())?;

    use_allowance(deps.storage, env, owner, spender, raw_amount)?;

    #[cfg(feature = "gas_tracking")]
    let mut tracker: GasTracker = GasTracker::new(deps.api);

    let owner_balance = perform_transfer(
        deps.storage,
        rng,
        &raw_owner,
        &raw_recipient,
        &raw_spender,
        raw_amount,
        denom,
        memo,
        &env.block,
        #[cfg(feature = "gas_tracking")]
        &mut tracker,
    )?;

    let received_notification = Notification::new(
        recipient.clone(),
        ReceivedNotificationData {
            amount: amount.u128(),
            sender: Some(owner.clone()),
        }
    );

    let spent_notification = Notification::new (
        owner.clone(),
        SpentNotificationData {
            amount: amount.u128(),
            actions: 1,
            recipient: Some(recipient.clone()),
            balance: owner_balance,
        }
    );

    Ok((received_notification, spent_notification))
}

#[allow(clippy::too_many_arguments)]
fn try_transfer_from(
    mut deps: DepsMut,
    env: &Env,
    info: MessageInfo,
    rng: &mut ContractPrng,
    owner: String,
    recipient: String,
    amount: Uint128,
    memo: Option<String>,
) -> StdResult<Response> {
    let secret = INTERNAL_SECRET.load(deps.storage)?;
    let secret = secret.as_slice();

    let owner = deps.api.addr_validate(owner.as_str())?;
    let recipient = deps.api.addr_validate(recipient.as_str())?;
    let symbol = CONFIG.load(deps.storage)?.symbol;
    let (received_notification, spent_notification) = try_transfer_from_impl(
        &mut deps,
        rng,
        env,
        &info.sender,
        &owner,
        &recipient,
        amount,
        symbol,
        memo,
    )?;
    let received_notification = received_notification.to_txhash_notification(
        deps.api,
        &env,
        secret,
        Some(NOTIFICATION_BLOCK_SIZE),
    )?;

    let spent_notification = spent_notification.to_txhash_notification(
        deps.api, 
        &env, 
        secret, 
        Some(NOTIFICATION_BLOCK_SIZE)
    )?;

    Ok(
        Response::new()
            .set_data(to_binary(&ExecuteAnswer::TransferFrom { status: Success })?)
            .add_attribute_plaintext(
                received_notification.id_plaintext(),
                received_notification.data_plaintext(),
            )
            .add_attribute_plaintext(
                spent_notification.id_plaintext(),
                spent_notification.data_plaintext(),
            )
    )
}

fn try_batch_transfer_from(
    mut deps: DepsMut,
    env: &Env,
    info: MessageInfo,
    rng: &mut ContractPrng,
    actions: Vec<batch::TransferFromAction>,
) -> StdResult<Response> {
    let secret = INTERNAL_SECRET.load(deps.storage)?;
    let secret = secret.as_slice();

    let mut notifications = vec![];

    let symbol = CONFIG.load(deps.storage)?.symbol;
    for action in actions {
        let owner = deps.api.addr_validate(action.owner.as_str())?;
        let recipient = deps.api.addr_validate(action.recipient.as_str())?;
        let (received_notification, spent_notification) = try_transfer_from_impl(
            &mut deps,
            rng,
            env,
            &info.sender,
            &owner,
            &recipient,
            action.amount,
            symbol.clone(),
            action.memo,
        )?;
        notifications.push((received_notification, spent_notification));
    }

    let tx_hash = env
        .transaction
        .clone()
        .ok_or(StdError::generic_err("no tx hash found"))?
        .hash;

    let (received_notifications, spent_notifications): (
        Vec<Notification<ReceivedNotificationData>>,
        Vec<Notification<SpentNotificationData>>,
    ) = notifications.into_iter().unzip();
    let received_data = multi_received_data(
        deps.api,
        received_notifications,
        &tx_hash,
        env.block.random.clone().unwrap(),
        secret,
    )?;
    let spent_data = multi_spent_data(
        deps.api,
        spent_notifications,
        &tx_hash,
        env.block.random.clone().unwrap(),
        secret,
    )?;

    Ok(
        Response::new()
            .set_data(to_binary(&ExecuteAnswer::BatchTransferFrom {status: Success})?)
            .add_attribute_plaintext(
                format!("snip52:#{}", MULTI_RECEIVED_CHANNEL_ID),
                Binary::from(received_data).to_base64(),
            )
            .add_attribute_plaintext(
                format!("snip52:#{}", MULTI_SPENT_CHANNEL_ID),
                Binary::from(spent_data).to_base64(),
            )
    )
}

#[allow(clippy::too_many_arguments)]
fn try_send_from_impl(
    deps: &mut DepsMut,
    env: Env,
    info: &MessageInfo,
    rng: &mut ContractPrng,
    messages: &mut Vec<CosmosMsg>,
    owner: Addr,
    recipient: Addr,
    recipient_code_hash: Option<String>,
    amount: Uint128,
    memo: Option<String>,
    msg: Option<Binary>,
) -> StdResult<(Notification<ReceivedNotificationData>, Notification<SpentNotificationData>)> {
    let spender = info.sender.clone();
    let symbol = CONFIG.load(deps.storage)?.symbol;
    let (received_notification, spent_notification) = try_transfer_from_impl(
        deps,
        rng,
        &env,
        &spender,
        &owner,
        &recipient,
        amount,
        symbol,
        memo.clone(),
    )?;

    try_add_receiver_api_callback(
        deps.storage,
        messages,
        recipient,
        recipient_code_hash,
        msg,
        info.sender.clone(),
        owner,
        amount,
        memo,
    )?;

    Ok((received_notification, spent_notification))
}

#[allow(clippy::too_many_arguments)]
fn try_send_from(
    mut deps: DepsMut,
    env: Env,
    info: &MessageInfo,
    rng: &mut ContractPrng,
    owner: String,
    recipient: String,
    recipient_code_hash: Option<String>,
    amount: Uint128,
    memo: Option<String>,
    msg: Option<Binary>,
) -> StdResult<Response> {
    let secret = INTERNAL_SECRET.load(deps.storage)?;
    let secret = secret.as_slice();

    let owner = deps.api.addr_validate(owner.as_str())?;
    let recipient = deps.api.addr_validate(recipient.as_str())?;
    let mut messages = vec![];
    let (received_notification, spent_notification) = try_send_from_impl(
        &mut deps,
        env.clone(),
        info,
        rng,
        &mut messages,
        owner,
        recipient,
        recipient_code_hash,
        amount,
        memo,
        msg,
    )?;

    let received_notification = received_notification.to_txhash_notification(
        deps.api,
        &env,
        secret,
        Some(NOTIFICATION_BLOCK_SIZE),
    )?;
    let spent_notification =
        spent_notification.to_txhash_notification(deps.api, &env, secret, Some(NOTIFICATION_BLOCK_SIZE))?;

    Ok(Response::new()
        .add_messages(messages)
        .set_data(to_binary(&ExecuteAnswer::SendFrom { status: Success })?)
        .add_attribute_plaintext(
            received_notification.id_plaintext(),
            received_notification.data_plaintext(),
        )
        .add_attribute_plaintext(
            spent_notification.id_plaintext(),
            spent_notification.data_plaintext(),
        )
    )
}

fn try_batch_send_from(
    mut deps: DepsMut,
    env: Env,
    info: &MessageInfo,
    rng: &mut ContractPrng,
    actions: Vec<batch::SendFromAction>,
) -> StdResult<Response> {
    let secret = INTERNAL_SECRET.load(deps.storage)?;
    let secret = secret.as_slice();

    let mut messages = vec![];
    let mut notifications = vec![];

    for action in actions {
        let owner = deps.api.addr_validate(action.owner.as_str())?;
        let recipient = deps.api.addr_validate(action.recipient.as_str())?;
        let (received_notification, spent_notification) = try_send_from_impl(
            &mut deps,
            env.clone(),
            info,
            rng,
            &mut messages,
            owner,
            recipient,
            action.recipient_code_hash,
            action.amount,
            action.memo,
            action.msg,
        )?;
        notifications.push((received_notification, spent_notification));
    }

    let tx_hash = env
        .transaction
        .clone()
        .ok_or(StdError::generic_err("no tx hash found"))?
        .hash;

    let (received_notifications, spent_notifications): (
        Vec<Notification<ReceivedNotificationData>>,
        Vec<Notification<SpentNotificationData>>,
    ) = notifications.into_iter().unzip();
    let received_data = multi_received_data(
        deps.api,
        received_notifications,
        &tx_hash,
        env.block.random.clone().unwrap(),
        secret,
    )?;
    let spent_data = multi_spent_data(
        deps.api,
        spent_notifications,
        &tx_hash,
        env.block.random.clone().unwrap(),
        secret,
    )?;

    Ok(Response::new()
        .add_messages(messages)
        .set_data(to_binary(&ExecuteAnswer::BatchSendFrom {
            status: Success,
        })?)
        .add_attribute_plaintext(
            format!("snip52:#{}", MULTI_RECEIVED_CHANNEL_ID),
            Binary::from(received_data).to_base64(),
        )
        .add_attribute_plaintext(
            format!("snip52:#{}", MULTI_SPENT_CHANNEL_ID),
            Binary::from(spent_data).to_base64(),
        )
    )
}

#[allow(clippy::too_many_arguments)]
fn try_burn_from(
    deps: DepsMut,
    env: &Env,
    info: MessageInfo,
    owner: String,
    amount: Uint128,
    memo: Option<String>,
) -> StdResult<Response> {
    let secret = INTERNAL_SECRET.load(deps.storage)?;
    let secret = secret.as_slice();
    
    let owner = deps.api.addr_validate(owner.as_str())?;
    let raw_owner = deps.api.addr_canonicalize(owner.as_str())?;
    let constants = CONFIG.load(deps.storage)?;
    if !constants.burn_is_enabled {
        return Err(StdError::generic_err(
            "Burn functionality is not enabled for this token.",
        ));
    }

    let raw_amount = amount.u128();
    use_allowance(deps.storage, env, &owner, &info.sender, raw_amount)?;
    let raw_burner = deps.api.addr_canonicalize(info.sender.as_str())?;

    let tx_id = store_burn_action(
        deps.storage,
        raw_owner.clone(),
        raw_burner.clone(),
        raw_amount,
        constants.symbol,
        memo,
        &env.block,
    )?;

    // load delayed write buffer
    let mut dwb = DWB.load(deps.storage)?;

    #[cfg(feature = "gas_tracking")]
    let mut tracker = GasTracker::new(deps.api);

    // settle the owner's account in buffer
    let owner_balance = dwb.settle_sender_or_owner_account(
        deps.storage,
        &raw_owner,
        tx_id,
        raw_amount,
        "burn",
        #[cfg(feature = "gas_tracking")]
        &mut tracker,
        &env.block,
    )?;
    if raw_burner != raw_owner {
        // also settle sender's account
        dwb.settle_sender_or_owner_account(
            deps.storage,
            &raw_burner,
            tx_id,
            0,
            "burn",
            #[cfg(feature = "gas_tracking")]
            &mut tracker,
            &env.block,
        )?;
    }

    DWB.save(deps.storage, &dwb)?;

    // remove from supply
    let mut total_supply = TOTAL_SUPPLY.load(deps.storage)?;

    if let Some(new_total_supply) = total_supply.checked_sub(raw_amount) {
        total_supply = new_total_supply;
    } else {
        return Err(StdError::generic_err(
            "You're trying to burn more than is available in the total supply",
        ));
    }
    TOTAL_SUPPLY.save(deps.storage, &total_supply)?;

    let spent_notification = Notification::new (
        owner,
        SpentNotificationData {
            amount: raw_amount,
            actions: 1,
            recipient: None,
            balance: owner_balance,
        }
    )
    .to_txhash_notification(deps.api, &env, secret, Some(NOTIFICATION_BLOCK_SIZE))?;

    Ok(
        Response::new()
            .set_data(to_binary(&ExecuteAnswer::BurnFrom { status: Success })?)
            .add_attribute_plaintext(
                spent_notification.id_plaintext(),
                spent_notification.data_plaintext(),
            )
    )
}

fn try_batch_burn_from(
    deps: DepsMut,
    env: &Env,
    info: MessageInfo,
    actions: Vec<batch::BurnFromAction>,
) -> StdResult<Response> {
    let secret = INTERNAL_SECRET.load(deps.storage)?;
    let secret = secret.as_slice();

    let constants = CONFIG.load(deps.storage)?;
    if !constants.burn_is_enabled {
        return Err(StdError::generic_err(
            "Burn functionality is not enabled for this token.",
        ));
    }

    let raw_spender = deps.api.addr_canonicalize(info.sender.as_str())?;
    let mut total_supply = TOTAL_SUPPLY.load(deps.storage)?;
    let mut spent_notifications = vec![];

    for action in actions {
        let owner = deps.api.addr_validate(action.owner.as_str())?;
        let raw_owner = deps.api.addr_canonicalize(owner.as_str())?;
        let amount = action.amount.u128();
        use_allowance(deps.storage, env, &owner, &info.sender, amount)?;

        let tx_id = store_burn_action(
            deps.storage,
            raw_owner.clone(),
            raw_spender.clone(),
            amount,
            constants.symbol.clone(),
            action.memo.clone(),
            &env.block,
        )?;

        // load delayed write buffer
        let mut dwb = DWB.load(deps.storage)?;

        #[cfg(feature = "gas_tracking")]
        let mut tracker = GasTracker::new(deps.api);

        // settle the owner's account in buffer
        let owner_balance = dwb.settle_sender_or_owner_account(
            deps.storage,
            &raw_owner,
            tx_id,
            amount,
            "burn",
            #[cfg(feature = "gas_tracking")]
            &mut tracker,
            &env.block,
        )?;
        if raw_spender != raw_owner {
            dwb.settle_sender_or_owner_account(
                deps.storage,
                &raw_spender,
                tx_id,
                0,
                "burn",
                #[cfg(feature = "gas_tracking")]
                &mut tracker,
                &env.block,
            )?;
        }

        DWB.save(deps.storage, &dwb)?;

        // remove from supply
        if let Some(new_total_supply) = total_supply.checked_sub(amount) {
            total_supply = new_total_supply;
        } else {
            return Err(StdError::generic_err(format!(
                "You're trying to burn more than is available in the total supply: {action:?}",
            )));
        }

        spent_notifications.push(Notification::new (
            info.sender.clone(),
            SpentNotificationData {
                amount,
                actions: 1,
                recipient: None,
                balance: owner_balance,
            }
        ));
    }

    TOTAL_SUPPLY.save(deps.storage, &total_supply)?;

    let tx_hash = env
        .transaction
        .clone()
        .ok_or(StdError::generic_err("no tx hash found"))?
        .hash;
    let spent_data = multi_spent_data(
        deps.api,
        spent_notifications,
        &tx_hash,
        env.block.random.clone().unwrap(),
        secret,
    )?;

    Ok(
        Response::new()
            .set_data(to_binary(&ExecuteAnswer::BatchBurnFrom {status: Success,})?)
            .add_attribute_plaintext(
                format!("snip52:#{}", MULTI_SPENT_CHANNEL_ID),
                Binary::from(spent_data).to_base64(),
            )
    )
}

fn try_increase_allowance(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    spender: String,
    amount: Uint128,
    expiration: Option<u64>,
) -> StdResult<Response> {
    let secret = INTERNAL_SECRET.load(deps.storage)?;
    let secret = secret.as_slice();

    let spender = deps.api.addr_validate(spender.as_str())?;
    let mut allowance = AllowancesStore::load(deps.storage, &info.sender, &spender);

    // If the previous allowance has expired, reset the allowance.
    // Without this users can take advantage of an expired allowance given to
    // them long ago.
    if allowance.is_expired_at(&env.block) {
        allowance.amount = amount.u128();
        allowance.expiration = None;
    } else {
        allowance.amount = allowance.amount.saturating_add(amount.u128());
    }

    if expiration.is_some() {
        allowance.expiration = expiration;
    }
    let new_amount = allowance.amount;
    AllowancesStore::save(deps.storage, &info.sender, &spender, &allowance)?;

    let notification = Notification::new (
        spender.clone(),
        AllowanceNotificationData {
            amount: new_amount,
            allower: info.sender.clone(),
            expiration,
        }
    )
    .to_txhash_notification(deps.api, &env, secret, Some(NOTIFICATION_BLOCK_SIZE))?;

    Ok(
        Response::new()
            .set_data(to_binary(&ExecuteAnswer::IncreaseAllowance {
                owner: info.sender,
                spender,
                allowance: Uint128::from(new_amount),
            })?)
            .add_attribute_plaintext(
                notification.id_plaintext(),
                notification.data_plaintext()
            )
    )
}

fn try_decrease_allowance(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    spender: String,
    amount: Uint128,
    expiration: Option<u64>,
) -> StdResult<Response> {
    let secret = INTERNAL_SECRET.load(deps.storage)?;
    let secret = secret.as_slice();

    let spender = deps.api.addr_validate(spender.as_str())?;
    let mut allowance = AllowancesStore::load(deps.storage, &info.sender, &spender);

    // If the previous allowance has expired, reset the allowance.
    // Without this users can take advantage of an expired allowance given to
    // them long ago.
    if allowance.is_expired_at(&env.block) {
        allowance.amount = 0;
        allowance.expiration = None;
    } else {
        allowance.amount = allowance.amount.saturating_sub(amount.u128());
    }

    if expiration.is_some() {
        allowance.expiration = expiration;
    }
    let new_amount = allowance.amount;
    AllowancesStore::save(deps.storage, &info.sender, &spender, &allowance)?;

    let notification = Notification::new (
        spender.clone(),
        AllowanceNotificationData {
            amount: new_amount,
            allower: info.sender.clone(),
            expiration,
        }
    )
    .to_txhash_notification(deps.api, &env, secret, Some(NOTIFICATION_BLOCK_SIZE))?;

    Ok(
        Response::new()
            .set_data(to_binary(&ExecuteAnswer::DecreaseAllowance {
                owner: info.sender,
                spender,
                allowance: Uint128::from(new_amount),
            })?)
            .add_attribute_plaintext(
                notification.id_plaintext(),
                notification.data_plaintext()
            )
    )
}

fn add_minters(
    deps: DepsMut,
    info: MessageInfo,
    minters_to_add: Vec<String>,
) -> StdResult<Response> {
    let constants = CONFIG.load(deps.storage)?;
    if !constants.mint_is_enabled {
        return Err(StdError::generic_err(
            "Mint functionality is not enabled for this token.",
        ));
    }

    check_if_admin(&constants.admin, &info.sender)?;

    let minters_to_add: Vec<Addr> = minters_to_add
        .iter()
        .map(|minter| deps.api.addr_validate(minter.as_str()).unwrap())
        .collect();
    MintersStore::add_minters(deps.storage, minters_to_add)?;

    Ok(Response::new().set_data(to_binary(&ExecuteAnswer::AddMinters { status: Success })?))
}

fn remove_minters(
    deps: DepsMut,
    info: MessageInfo,
    minters_to_remove: Vec<String>,
) -> StdResult<Response> {
    let constants = CONFIG.load(deps.storage)?;
    if !constants.mint_is_enabled {
        return Err(StdError::generic_err(
            "Mint functionality is not enabled for this token.",
        ));
    }

    check_if_admin(&constants.admin, &info.sender)?;

    let minters_to_remove: StdResult<Vec<Addr>> = minters_to_remove
        .iter()
        .map(|minter| deps.api.addr_validate(minter.as_str()))
        .collect();
    MintersStore::remove_minters(deps.storage, minters_to_remove?)?;

    Ok(
        Response::new().set_data(to_binary(&ExecuteAnswer::RemoveMinters {
            status: Success,
        })?),
    )
}

fn set_minters(
    deps: DepsMut,
    info: MessageInfo,
    minters_to_set: Vec<String>,
) -> StdResult<Response> {
    let constants = CONFIG.load(deps.storage)?;
    if !constants.mint_is_enabled {
        return Err(StdError::generic_err(
            "Mint functionality is not enabled for this token.",
        ));
    }

    check_if_admin(&constants.admin, &info.sender)?;

    let minters_to_set: Vec<Addr> = minters_to_set
        .iter()
        .map(|minter| deps.api.addr_validate(minter.as_str()).unwrap())
        .collect();
    MintersStore::save(deps.storage, minters_to_set)?;

    Ok(Response::new().set_data(to_binary(&ExecuteAnswer::SetMinters { status: Success })?))
}

/// Burn tokens
///
/// Remove `amount` tokens from the system irreversibly, from signer account
///
/// @param amount the amount of money to burn
fn try_burn(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    amount: Uint128,
    memo: Option<String>,
) -> StdResult<Response> {
    let secret = INTERNAL_SECRET.load(deps.storage)?;
    let secret = secret.as_slice();

    let constants = CONFIG.load(deps.storage)?;
    if !constants.burn_is_enabled {
        return Err(StdError::generic_err(
            "Burn functionality is not enabled for this token.",
        ));
    }

    let raw_amount = amount.u128();
    let raw_burn_address = deps.api.addr_canonicalize(info.sender.as_str())?;

    let tx_id = store_burn_action(
        deps.storage,
        raw_burn_address.clone(),
        raw_burn_address.clone(),
        raw_amount,
        constants.symbol,
        memo,
        &env.block,
    )?;

    // load delayed write buffer
    let mut dwb = DWB.load(deps.storage)?;

    #[cfg(feature = "gas_tracking")]
    let mut tracker = GasTracker::new(deps.api);

    // settle the signer's account in buffer
    let owner_balance = dwb.settle_sender_or_owner_account(
        deps.storage,
        &raw_burn_address,
        tx_id,
        raw_amount,
        "burn",
        #[cfg(feature = "gas_tracking")]
        &mut tracker,
        &env.block,
    )?;

    DWB.save(deps.storage, &dwb)?;

    let mut total_supply = TOTAL_SUPPLY.load(deps.storage)?;
    if let Some(new_total_supply) = total_supply.checked_sub(raw_amount) {
        total_supply = new_total_supply;
    } else {
        return Err(StdError::generic_err(
            "You're trying to burn more than is available in the total supply",
        ));
    }
    TOTAL_SUPPLY.save(deps.storage, &total_supply)?;

    let spent_notification = Notification::new (
        info.sender,
        SpentNotificationData {
            amount: raw_amount,
            actions: 1,
            recipient: None,
            balance: owner_balance,
        }
    )
    .to_txhash_notification(deps.api, &env, secret, Some(NOTIFICATION_BLOCK_SIZE))?;

    Ok(
        Response::new()
            .set_data(to_binary(&ExecuteAnswer::Burn { status: Success })?)
            .add_attribute_plaintext(
                spent_notification.id_plaintext(),
                spent_notification.data_plaintext(),
            )
    )
}

fn perform_transfer(
    store: &mut dyn Storage,
    rng: &mut ContractPrng,
    from: &CanonicalAddr,
    to: &CanonicalAddr,
    sender: &CanonicalAddr,
    amount: u128,
    denom: String,
    memo: Option<String>,
    block: &BlockInfo,
    #[cfg(feature = "gas_tracking")] tracker: &mut GasTracker,
) -> StdResult<u128> {
    #[cfg(feature = "gas_tracking")]
    let mut group1 = tracker.group("perform_transfer.1");

    // first store the tx information in the global append list of txs and get the new tx id
    let tx_id = store_transfer_action(store, from, sender, to, amount, denom, memo, block)?;

    #[cfg(feature = "gas_tracking")]
    group1.log("@store_transfer_action");

    // load delayed write buffer
    let mut dwb = DWB.load(store)?;

    #[cfg(feature = "gas_tracking")]
    group1.log("DWB.load");

    let transfer_str = "transfer";

    // settle the owner's account
    let owner_balance = dwb.settle_sender_or_owner_account(
        store,
        from,
        tx_id,
        amount,
        transfer_str,
        #[cfg(feature = "gas_tracking")]
        tracker,
        block,
    )?;

    // if this is a *_from action, settle the sender's account, too
    if sender != from {
        dwb.settle_sender_or_owner_account(
            store,
            sender,
            tx_id,
            0,
            transfer_str,
            #[cfg(feature = "gas_tracking")]
            tracker,
            block,
        )?;
    }

    // add the tx info for the recipient to the buffer
    dwb.add_recipient(
        store,
        rng,
        to,
        tx_id,
        amount,
        #[cfg(feature = "gas_tracking")]
        tracker,
        block,
    )?;

    #[cfg(feature = "gas_tracking")]
    let mut group2 = tracker.group("perform_transfer.2");

    DWB.save(store, &dwb)?;

    #[cfg(feature = "gas_tracking")]
    group2.log("DWB.save");

    Ok(owner_balance)
}

fn perform_mint(
    store: &mut dyn Storage,
    rng: &mut ContractPrng,
    minter: &CanonicalAddr,
    to: &CanonicalAddr,
    amount: u128,
    denom: String,
    memo: Option<String>,
    block: &BlockInfo,
    #[cfg(feature = "gas_tracking")] tracker: &mut GasTracker,
) -> StdResult<()> {
    // first store the tx information in the global append list of txs and get the new tx id
    let tx_id = store_mint_action(store, minter, to, amount, denom, memo, block)?;

    // load delayed write buffer
    let mut dwb = DWB.load(store)?;

    // if minter is not recipient, settle them
    if minter != to {
        dwb.settle_sender_or_owner_account(
            store,
            minter,
            tx_id,
            0,
            "mint",
            #[cfg(feature = "gas_tracking")]
            tracker,
            block,
        )?;
    }

    // add the tx info for the recipient to the buffer
    dwb.add_recipient(
        store,
        rng,
        to,
        tx_id,
        amount,
        #[cfg(feature = "gas_tracking")]
        tracker,
        block,
    )?;

    DWB.save(store, &dwb)?;

    Ok(())
}

fn perform_deposit(
    store: &mut dyn Storage,
    rng: &mut ContractPrng,
    to: &CanonicalAddr,
    amount: u128,
    denom: String,
    block: &BlockInfo,
    #[cfg(feature = "gas_tracking")] tracker: &mut GasTracker,
) -> StdResult<()> {
    // first store the tx information in the global append list of txs and get the new tx id
    let tx_id = store_deposit_action(store, amount, denom, block)?;

    // load delayed write buffer
    let mut dwb = DWB.load(store)?;

    // add the tx info for the recipient to the buffer
    dwb.add_recipient(
        store,
        rng,
        to,
        tx_id,
        amount,
        #[cfg(feature = "gas_tracking")]
        tracker,
        block,
    )?;

    DWB.save(store, &dwb)?;

    Ok(())
}

fn revoke_permit(deps: DepsMut, info: MessageInfo, permit_name: String) -> StdResult<Response> {
    RevokedPermits::revoke_permit(
        deps.storage,
        PREFIX_REVOKED_PERMITS,
        info.sender.as_str(),
        &permit_name,
    );

    Ok(Response::new().set_data(to_binary(&ExecuteAnswer::RevokePermit { status: Success })?))
}

fn check_if_admin(config_admin: &Addr, account: &Addr) -> StdResult<()> {
    if config_admin != account {
        return Err(StdError::generic_err(
            "This is an admin command. Admin commands can only be run from admin address",
        ));
    }

    Ok(())
}

fn is_valid_name(name: &str) -> bool {
    let len = name.len();
    (3..=30).contains(&len)
}

fn is_valid_symbol(symbol: &str) -> bool {
    let len = symbol.len();
    let len_is_valid = (3..=20).contains(&len);

    len_is_valid && symbol.bytes().all(|byte| byte.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use std::any::Any;

    use cosmwasm_std::{
        from_binary, testing::*, Api, BlockInfo, ContractInfo, MessageInfo, OwnedDeps,
        QueryResponse, ReplyOn, SubMsg, Timestamp, TransactionInfo, WasmMsg,
    };
    use secret_toolkit::permit::{PermitParams, PermitSignature, PubKey};

    use crate::dwb::TX_NODES_COUNT;
    use crate::msg::{InitConfig, InitialBalance, ResponseStatus};
    use crate::state::TX_COUNT;
    use crate::transaction_history::TxAction;

    use super::*;

    pub const VIEWING_KEY_SIZE: usize = 32;

    // Helper functions

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

    fn init_helper_with_config(
        initial_balances: Vec<InitialBalance>,
        enable_deposit: bool,
        enable_redeem: bool,
        enable_mint: bool,
        enable_burn: bool,
        contract_bal: u128,
        supported_denoms: Vec<String>,
    ) -> (
        StdResult<Response>,
        OwnedDeps<MockStorage, MockApi, MockQuerier>,
    ) {
        let mut deps = mock_dependencies_with_balance(&[Coin {
            denom: "uscrt".to_string(),
            amount: Uint128::new(contract_bal),
        }]);

        let env = mock_env();
        let info = mock_info("instantiator", &[]);

        let init_config: InitConfig = from_binary(&Binary::from(
            format!(
                "{{\"public_total_supply\":false,
            \"enable_deposit\":{},
            \"enable_redeem\":{},
            \"enable_mint\":{},
            \"enable_burn\":{}}}",
                enable_deposit, enable_redeem, enable_mint, enable_burn
            )
            .as_bytes(),
        ))
        .unwrap();
        let init_msg = InstantiateMsg {
            name: "sec-sec".to_string(),
            admin: Some("admin".to_string()),
            symbol: "SECSEC".to_string(),
            decimals: 8,
            initial_balances: Some(initial_balances),
            prng_seed: Binary::from("lolz fun yay".as_bytes()),
            config: Some(init_config),
            supported_denoms: Some(supported_denoms),
        };

        (instantiate(deps.as_mut(), env, info, init_msg), deps)
    }

    fn extract_error_msg<T: Any>(error: StdResult<T>) -> String {
        match error {
            Ok(response) => {
                let bin_err = (&response as &dyn Any)
                    .downcast_ref::<QueryResponse>()
                    .expect("An error was expected, but no error could be extracted");
                match from_binary(bin_err).unwrap() {
                    QueryAnswer::ViewingKeyError { msg } => msg,
                    _ => panic!("Unexpected query answer"),
                }
            }
            Err(err) => match err {
                StdError::GenericErr { msg, .. } => msg,
                _ => panic!("Unexpected result from init"),
            },
        }
    }

    fn ensure_success(handle_result: Response) -> bool {
        let handle_result: ExecuteAnswer = from_binary(&handle_result.data.unwrap()).unwrap();

        match handle_result {
            ExecuteAnswer::Deposit { status }
            | ExecuteAnswer::Redeem { status }
            | ExecuteAnswer::Transfer { status }
            | ExecuteAnswer::Send { status }
            | ExecuteAnswer::Burn { status }
            | ExecuteAnswer::RegisterReceive { status }
            | ExecuteAnswer::SetViewingKey { status }
            | ExecuteAnswer::TransferFrom { status }
            | ExecuteAnswer::SendFrom { status }
            | ExecuteAnswer::BurnFrom { status }
            | ExecuteAnswer::Mint { status }
            | ExecuteAnswer::ChangeAdmin { status }
            | ExecuteAnswer::SetContractStatus { status }
            | ExecuteAnswer::SetMinters { status }
            | ExecuteAnswer::AddMinters { status }
            | ExecuteAnswer::RemoveMinters { status } => {
                matches!(status, ResponseStatus::Success { .. })
            }
            _ => panic!(
                "HandleAnswer not supported for success extraction: {:?}",
                handle_result
            ),
        }
    }

    /// creates a cosmos_msg sending this struct to the named contract
    pub fn into_cosmos_submsg(
        msg: Snip20ReceiveMsg,
        code_hash: String,
        contract_addr: Addr,
        id: u64,
    ) -> StdResult<SubMsg> {
        let msg = msg.into_binary()?;
        let execute = SubMsg {
            id,
            msg: WasmMsg::Execute {
                contract_addr: contract_addr.into_string(),
                code_hash,
                msg,
                funds: vec![],
            }
            .into(),
            // TODO: Discuss the wanted behavior
            reply_on: match id {
                0 => ReplyOn::Never,
                _ => ReplyOn::Always,
            },
            gas_limit: None,
        };

        Ok(execute)
    }

    // Init tests

    #[test]
    fn test_init_sanity() {
        let (init_result, mut deps) = init_helper(vec![InitialBalance {
            address: "lebron".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert_eq!(init_result.unwrap(), Response::default());

        let constants = CONFIG.load(&deps.storage).unwrap();
        assert_eq!(TOTAL_SUPPLY.load(&deps.storage).unwrap(), 5000);
        assert_eq!(
            CONTRACT_STATUS.load(&deps.storage).unwrap(),
            ContractStatusLevel::NormalRun
        );
        assert_eq!(constants.name, "sec-sec".to_string());
        assert_eq!(constants.admin, Addr::unchecked("admin".to_string()));
        assert_eq!(constants.symbol, "SECSEC".to_string());
        assert_eq!(constants.decimals, 8);
        assert_eq!(constants.total_supply_is_public, false);

        ViewingKey::set(deps.as_mut().storage, "lebron", "lolz fun yay");
        let is_vk_correct = ViewingKey::check(&deps.storage, "lebron", "lolz fun yay");
        assert!(
            is_vk_correct.is_ok(),
            "Viewing key verification failed!: {}",
            is_vk_correct.err().unwrap()
        );
    }

    #[test]
    fn test_init_with_config_sanity() {
        let (init_result, mut deps) = init_helper_with_config(
            vec![InitialBalance {
                address: "lebron".to_string(),
                amount: Uint128::new(5000),
            }],
            true,
            true,
            true,
            true,
            0,
            vec!["uscrt".to_string()],
        );
        assert_eq!(init_result.unwrap(), Response::default());

        let constants = CONFIG.load(&deps.storage).unwrap();
        assert_eq!(TOTAL_SUPPLY.load(&deps.storage).unwrap(), 5000);
        assert_eq!(
            CONTRACT_STATUS.load(&deps.storage).unwrap(),
            ContractStatusLevel::NormalRun
        );
        assert_eq!(constants.name, "sec-sec".to_string());
        assert_eq!(constants.admin, Addr::unchecked("admin".to_string()));
        assert_eq!(constants.symbol, "SECSEC".to_string());
        assert_eq!(constants.decimals, 8);
        assert_eq!(constants.total_supply_is_public, false);
        assert_eq!(constants.deposit_is_enabled, true);
        assert_eq!(constants.redeem_is_enabled, true);
        assert_eq!(constants.mint_is_enabled, true);
        assert_eq!(constants.burn_is_enabled, true);

        ViewingKey::set(deps.as_mut().storage, "lebron", "lolz fun yay");
        let is_vk_correct = ViewingKey::check(&deps.storage, "lebron", "lolz fun yay");
        assert!(
            is_vk_correct.is_ok(),
            "Viewing key verification failed!: {}",
            is_vk_correct.err().unwrap()
        );
    }

    #[test]
    fn test_total_supply_overflow_dwb() {
        // with this implementation of dwbs the max amount a user can get transferred or minted is u64::MAX
        // for 18 digit coins, u128 amounts might be stored in the dwb (see `fn add_amount` in dwb.rs)
        let (init_result, _deps) = init_helper(vec![InitialBalance {
            address: "lebron".to_string(),
            amount: Uint128::new(u64::max_value().into()),
        }]);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );
    }

    // Handle tests

    #[test]
    fn test_execute_transfer_dwb() {
        let (init_result, mut deps) = init_helper(vec![InitialBalance {
            address: "bob".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let tx_nodes_count = TX_NODES_COUNT.load(&deps.storage).unwrap_or_default();
        // should be 2 because we minted 5000 to bob at initialization
        assert_eq!(2, tx_nodes_count);
        let tx_count = TX_COUNT.load(&deps.storage).unwrap_or_default();
        assert_eq!(1, tx_count); // due to mint

        let handle_msg = ExecuteMsg::Transfer {
            recipient: "alice".to_string(),
            amount: Uint128::new(1000),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);
        let mut env = mock_env();
        env.block.random = Some(Binary::from(&[0u8; 32]));
        let handle_result = execute(deps.as_mut(), env, info, handle_msg);

        let result = handle_result.unwrap();
        assert!(ensure_success(result));
        let bob_addr = deps
            .api
            .addr_canonicalize(Addr::unchecked("bob").as_str())
            .unwrap();
        let alice_addr = deps
            .api
            .addr_canonicalize(Addr::unchecked("alice").as_str())
            .unwrap();

        assert_eq!(
            5000 - 1000,
            stored_balance(&deps.storage, &bob_addr).unwrap().unwrap_or_default()
        );
        // alice has not been settled yet
        assert_ne!(1000, stored_balance(&deps.storage, &alice_addr).unwrap().unwrap_or_default());

        let dwb = DWB.load(&deps.storage).unwrap();
        println!("DWB: {dwb:?}");
        // assert we have decremented empty_space_counter
        assert_eq!(62, dwb.empty_space_counter);
        // assert first entry has correct information for alice
        let alice_entry = dwb.entries[2];
        assert_eq!(1, alice_entry.list_len().unwrap());
        assert_eq!(1000, alice_entry.amount().unwrap());
        // the id of the head_node
        assert_eq!(4, alice_entry.head_node().unwrap());
        let tx_count = TX_COUNT.load(&deps.storage).unwrap_or_default();
        assert_eq!(2, tx_count);

        // now send 100 to charlie from bob
        let handle_msg = ExecuteMsg::Transfer {
            recipient: "charlie".to_string(),
            amount: Uint128::new(100),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);

        let mut env = mock_env();
        env.block.random = Some(Binary::from(&[1u8; 32]));
        let handle_result = execute(deps.as_mut(), env, info, handle_msg);

        let result = handle_result.unwrap();
        assert!(ensure_success(result));
        let charlie_addr = deps
            .api
            .addr_canonicalize(Addr::unchecked("charlie").as_str())
            .unwrap();

        assert_eq!(
            5000 - 1000 - 100,
            stored_balance(&deps.storage, &bob_addr).unwrap().unwrap_or_default()
        );
        // alice has not been settled yet
        assert_ne!(1000, stored_balance(&deps.storage, &alice_addr).unwrap().unwrap_or_default());
        // charlie has not been settled yet
        assert_ne!(100, stored_balance(&deps.storage, &charlie_addr).unwrap().unwrap_or_default());

        let dwb = DWB.load(&deps.storage).unwrap();
        //println!("DWB: {dwb:?}");
        // assert we have decremented empty_space_counter
        assert_eq!(61, dwb.empty_space_counter);
        // assert entry has correct information for charlie
        let charlie_entry = dwb.entries[3];
        assert_eq!(1, charlie_entry.list_len().unwrap());
        assert_eq!(100, charlie_entry.amount().unwrap());
        // the id of the head_node
        assert_eq!(6, charlie_entry.head_node().unwrap());
        let tx_count = TX_COUNT.load(&deps.storage).unwrap_or_default();
        assert_eq!(3, tx_count);

        // send another 500 to alice from bob
        let handle_msg = ExecuteMsg::Transfer {
            recipient: "alice".to_string(),
            amount: Uint128::new(500),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);
        let mut env = mock_env();
        env.block.random = Some(Binary::from(&[2u8; 32]));
        let handle_result = execute(deps.as_mut(), env, info, handle_msg);

        let result = handle_result.unwrap();
        assert!(ensure_success(result));

        assert_eq!(
            5000 - 1000 - 100 - 500,
            stored_balance(&deps.storage, &bob_addr).unwrap().unwrap_or_default()
        );
        // make sure alice has not been settled yet
        assert_ne!(1500, stored_balance(&deps.storage, &alice_addr).unwrap().unwrap_or_default());

        let dwb = DWB.load(&deps.storage).unwrap();
        //println!("DWB: {dwb:?}");
        // assert we have not decremented empty_space_counter
        assert_eq!(61, dwb.empty_space_counter);
        // assert entry has correct information for alice
        let alice_entry = dwb.entries[2];
        assert_eq!(2, alice_entry.list_len().unwrap());
        assert_eq!(1500, alice_entry.amount().unwrap());
        // the id of the head_node
        assert_eq!(8, alice_entry.head_node().unwrap());
        let tx_count = TX_COUNT.load(&deps.storage).unwrap_or_default();
        assert_eq!(4, tx_count);

        // convert head_node to vec
        let alice_nodes = TX_NODES
            .add_suffix(&alice_entry.head_node().unwrap().to_be_bytes())
            .load(&deps.storage)
            .unwrap()
            .to_vec(&deps.storage, &deps.api)
            .unwrap();

        let expected_alice_nodes: Vec<Tx> = vec![
            Tx {
                id: 4,
                action: TxAction::Transfer {
                    from: Addr::unchecked("bob"),
                    sender: Addr::unchecked("bob"),
                    recipient: Addr::unchecked("alice"),
                },
                coins: Coin {
                    amount: Uint128::from(500_u128),
                    denom: "SECSEC".to_string(),
                },
                memo: None,
                block_time: 1571797419,
                block_height: 12345,
            },
            Tx {
                id: 2,
                action: TxAction::Transfer {
                    from: Addr::unchecked("bob"),
                    sender: Addr::unchecked("bob"),
                    recipient: Addr::unchecked("alice"),
                },
                coins: Coin {
                    amount: Uint128::from(1000_u128),
                    denom: "SECSEC".to_string(),
                },
                memo: None,
                block_time: 1571797419,
                block_height: 12345,
            },
        ];
        assert_eq!(alice_nodes, expected_alice_nodes);

        // now send 200 to ernie from bob
        let handle_msg = ExecuteMsg::Transfer {
            recipient: "ernie".to_string(),
            amount: Uint128::new(200),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);

        let mut env = mock_env();
        env.block.random = Some(Binary::from(&[3u8; 32]));
        let handle_result = execute(deps.as_mut(), env, info, handle_msg);

        let result = handle_result.unwrap();
        assert!(ensure_success(result));
        let ernie_addr = deps
            .api
            .addr_canonicalize(Addr::unchecked("ernie").as_str())
            .unwrap();

        assert_eq!(
            5000 - 1000 - 100 - 500 - 200,
            stored_balance(&deps.storage, &bob_addr).unwrap().unwrap_or_default()
        );
        // alice has not been settled yet
        assert_ne!(1500, stored_balance(&deps.storage, &alice_addr).unwrap().unwrap_or_default());
        // charlie has not been settled yet
        assert_ne!(100, stored_balance(&deps.storage, &charlie_addr).unwrap().unwrap_or_default());
        // ernie has not been settled yet
        assert_ne!(200, stored_balance(&deps.storage, &ernie_addr).unwrap().unwrap_or_default());

        let dwb = DWB.load(&deps.storage).unwrap();
        //println!("DWB: {dwb:?}");

        // assert we have decremented empty_space_counter
        assert_eq!(60, dwb.empty_space_counter);
        // assert entry has correct information for ernie
        let ernie_entry = dwb.entries[4];
        assert_eq!(1, ernie_entry.list_len().unwrap());
        assert_eq!(200, ernie_entry.amount().unwrap());
        // the id of the head_node
        assert_eq!(10, ernie_entry.head_node().unwrap());
        let tx_count = TX_COUNT.load(&deps.storage).unwrap_or_default();
        assert_eq!(5, tx_count);

        // now alice sends 50 to dora
        // this should settle alice and create entry for dora
        let handle_msg = ExecuteMsg::Transfer {
            recipient: "dora".to_string(),
            amount: Uint128::new(50),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("alice", &[]);
        let mut env = mock_env();
        env.block.random = Some(Binary::from(&[4u8; 32]));
        let handle_result = execute(deps.as_mut(), env, info, handle_msg);

        let result = handle_result.unwrap();
        assert!(ensure_success(result));
        let dora_addr = deps
            .api
            .addr_canonicalize(Addr::unchecked("dora").as_str())
            .unwrap();

        // alice has been settled
        assert_eq!(
            1500 - 50,
            stored_balance(&deps.storage, &alice_addr).unwrap().unwrap_or_default()
        );
        // dora has not been settled
        assert_ne!(50, stored_balance(&deps.storage, &dora_addr).unwrap().unwrap_or_default());

        let dwb = DWB.load(&deps.storage).unwrap();
        //println!("DWB: {dwb:?}");

        // assert we have decremented empty_space_counter
        assert_eq!(59, dwb.empty_space_counter);
        // assert entry has correct information for ernie
        let dora_entry = dwb.entries[5];
        assert_eq!(1, dora_entry.list_len().unwrap());
        assert_eq!(50, dora_entry.amount().unwrap());
        // the id of the head_node
        assert_eq!(12, dora_entry.head_node().unwrap());
        let tx_count = TX_COUNT.load(&deps.storage).unwrap_or_default();
        assert_eq!(6, tx_count);

        // now we will send to 60 more addresses to fill up the buffer
        for i in 1..=59 {
            let recipient = format!("receipient{i}");
            // now send 1 to recipient from bob
            let handle_msg = ExecuteMsg::Transfer {
                recipient,
                amount: Uint128::new(1),
                memo: None,
                #[cfg(feature = "gas_evaporation")]
                gas_target: None,
                padding: None,
            };
            let info = mock_info("bob", &[]);
            let mut env = mock_env();
            env.block.random = Some(Binary::from(&[255 - i; 32]));
            let handle_result = execute(deps.as_mut(), env, info, handle_msg);

            let result = handle_result.unwrap();
            assert!(ensure_success(result));
        }
        assert_eq!(
            5000 - 1000 - 100 - 500 - 200 - 59,
            stored_balance(&deps.storage, &bob_addr).unwrap().unwrap_or_default()
        );

        let dwb = DWB.load(&deps.storage).unwrap();
        //println!("DWB: {dwb:?}");

        // assert we have filled the buffer
        assert_eq!(0, dwb.empty_space_counter);

        let recipient = format!("receipient_over");
        // now send 1 to recipient from bob
        let handle_msg = ExecuteMsg::Transfer {
            recipient,
            amount: Uint128::new(1),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);
        let mut env = mock_env();
        env.block.random = Some(Binary::from(&[50; 32]));
        let handle_result = execute(deps.as_mut(), env, info, handle_msg);

        let result = handle_result.unwrap();
        assert!(ensure_success(result));

        assert_eq!(
            5000 - 1000 - 100 - 500 - 200 - 59 - 1,
            stored_balance(&deps.storage, &bob_addr).unwrap().unwrap_or_default()
        );

        //let dwb = DWB.load(&deps.storage).unwrap();
        //println!("DWB: {dwb:?}");

        let recipient = format!("receipient_over_2");
        // now send 1 to recipient from bob
        let handle_msg = ExecuteMsg::Transfer {
            recipient,
            amount: Uint128::new(1),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);
        let mut env = mock_env();
        env.block.random = Some(Binary::from(&[12; 32]));
        let handle_result = execute(deps.as_mut(), env, info, handle_msg);

        let result = handle_result.unwrap();
        assert!(ensure_success(result));

        assert_eq!(
            5000 - 1000 - 100 - 500 - 200 - 59 - 1 - 1,
            stored_balance(&deps.storage, &bob_addr).unwrap().unwrap_or_default()
        );

        //let dwb = DWB.load(&deps.storage).unwrap();
        //println!("DWB: {dwb:?}");

        // now we send 50 transactions to alice from bob
        for i in 1..=50 {
            // send 1 to alice from bob
            let handle_msg = ExecuteMsg::Transfer {
                recipient: "alice".to_string(),
                amount: Uint128::new(i.into()),
                memo: None,
                #[cfg(feature = "gas_evaporation")]
                gas_target: None,
                padding: None,
            };

            let info = mock_info("bob", &[]);
            let mut env = mock_env();
            env.block.random = Some(Binary::from(&[125 - i; 32]));
            let handle_result = execute(deps.as_mut(), env, info, handle_msg);

            let result = handle_result.unwrap();
            assert!(ensure_success(result));

            // alice should not settle
            assert_eq!(
                1500 - 50,
                stored_balance(&deps.storage, &alice_addr).unwrap().unwrap_or_default()
            );
        }

        // alice sends 1 to dora to settle
        // this should settle alice and create entry for dora
        let handle_msg = ExecuteMsg::Transfer {
            recipient: "dora".to_string(),
            amount: Uint128::new(1),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("alice", &[]);
        let mut env = mock_env();
        env.block.random = Some(Binary::from(&[61; 32]));
        let handle_result = execute(deps.as_mut(), env, info, handle_msg);

        let result = handle_result.unwrap();
        assert!(ensure_success(result));

        assert_eq!(2724, stored_balance(&deps.storage, &alice_addr).unwrap().unwrap_or_default());

        // now we send 50 more transactions to alice from bob
        for i in 1..=50 {
            // send 1 to alice from bob
            let handle_msg = ExecuteMsg::Transfer {
                recipient: "alice".to_string(),
                amount: Uint128::new(i.into()),
                memo: None,
                #[cfg(feature = "gas_evaporation")]
                gas_target: None,
                padding: None,
            };

            let info = mock_info("bob", &[]);
            let mut env = mock_env();
            env.block.random = Some(Binary::from(&[200 - i; 32]));
            let handle_result = execute(deps.as_mut(), env, info, handle_msg);

            let result = handle_result.unwrap();
            assert!(ensure_success(result));

            // alice should not settle
            assert_eq!(2724, stored_balance(&deps.storage, &alice_addr).unwrap().unwrap_or_default());
        }

        let handle_msg = ExecuteMsg::SetViewingKey {
            key: "key".to_string(),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("alice", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);
        let result = handle_result.unwrap();
        assert!(ensure_success(result));

        // check that alice's balance when queried is correct (includes both settled and dwb amounts)
        // settled = 2724
        // dwb = 1275
        // total should be = 3999
        let query_msg = QueryMsg::Balance {
            address: "alice".to_string(),
            key: "key".to_string(),
        };
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        let balance = match from_binary(&query_result.unwrap()).unwrap() {
            QueryAnswer::Balance { amount } => amount,
            _ => panic!("Unexpected"),
        };
        assert_eq!(balance, Uint128::new(3999));

        // now we use alice to check query transaction history pagination works

        //
        // check last 3 transactions for alice (all in dwb)
        //
        let query_msg = QueryMsg::TransactionHistory {
            address: "alice".to_string(),
            key: "key".to_string(),
            page: None,
            page_size: 3,
        };
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        let transfers = match from_binary(&query_result.unwrap()).unwrap() {
            QueryAnswer::TransactionHistory { txs, .. } => txs,
            other => panic!("Unexpected: {:?}", other),
        };
        //println!("transfers: {transfers:?}");
        let expected_transfers = vec![
            Tx {
                id: 168,
                action: TxAction::Transfer {
                    from: Addr::unchecked("bob"),
                    sender: Addr::unchecked("bob"),
                    recipient: Addr::unchecked("alice"),
                },
                coins: Coin {
                    denom: "SECSEC".to_string(),
                    amount: Uint128::from(50u128),
                },
                memo: None,
                block_time: 1571797419,
                block_height: 12345,
            },
            Tx {
                id: 167,
                action: TxAction::Transfer {
                    from: Addr::unchecked("bob"),
                    sender: Addr::unchecked("bob"),
                    recipient: Addr::unchecked("alice"),
                },
                coins: Coin {
                    denom: "SECSEC".to_string(),
                    amount: Uint128::from(49u128),
                },
                memo: None,
                block_time: 1571797419,
                block_height: 12345,
            },
            Tx {
                id: 166,
                action: TxAction::Transfer {
                    from: Addr::unchecked("bob"),
                    sender: Addr::unchecked("bob"),
                    recipient: Addr::unchecked("alice"),
                },
                coins: Coin {
                    denom: "SECSEC".to_string(),
                    amount: Uint128::from(48u128),
                },
                memo: None,
                block_time: 1571797419,
                block_height: 12345,
            },
        ];
        assert_eq!(transfers, expected_transfers);

        //
        // check 6 transactions for alice that span over end of the 50 in dwb and settled
        // page: 8, page size: 6
        // start is index 48
        //
        let query_msg = QueryMsg::TransactionHistory {
            address: "alice".to_string(),
            key: "key".to_string(),
            page: Some(8),
            page_size: 6,
        };
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        let transfers = match from_binary(&query_result.unwrap()).unwrap() {
            QueryAnswer::TransactionHistory { txs, .. } => txs,
            other => panic!("Unexpected: {:?}", other),
        };
        //println!("transfers: {transfers:?}");
        let expected_transfers = vec![
            Tx {
                id: 120,
                action: TxAction::Transfer {
                    from: Addr::unchecked("bob"),
                    sender: Addr::unchecked("bob"),
                    recipient: Addr::unchecked("alice"),
                },
                coins: Coin {
                    denom: "SECSEC".to_string(),
                    amount: Uint128::from(2u128),
                },
                memo: None,
                block_time: 1571797419,
                block_height: 12345,
            },
            Tx {
                id: 119,
                action: TxAction::Transfer {
                    from: Addr::unchecked("bob"),
                    sender: Addr::unchecked("bob"),
                    recipient: Addr::unchecked("alice"),
                },
                coins: Coin {
                    denom: "SECSEC".to_string(),
                    amount: Uint128::from(1u128),
                },
                memo: None,
                block_time: 1571797419,
                block_height: 12345,
            },
            Tx {
                id: 118,
                action: TxAction::Transfer {
                    from: Addr::unchecked("alice"),
                    sender: Addr::unchecked("alice"),
                    recipient: Addr::unchecked("dora"),
                },
                coins: Coin {
                    denom: "SECSEC".to_string(),
                    amount: Uint128::from(1u128),
                },
                memo: None,
                block_time: 1571797419,
                block_height: 12345,
            },
            Tx {
                id: 117,
                action: TxAction::Transfer {
                    from: Addr::unchecked("bob"),
                    sender: Addr::unchecked("bob"),
                    recipient: Addr::unchecked("alice"),
                },
                coins: Coin {
                    denom: "SECSEC".to_string(),
                    amount: Uint128::from(50u128),
                },
                memo: None,
                block_time: 1571797419,
                block_height: 12345,
            },
            Tx {
                id: 116,
                action: TxAction::Transfer {
                    from: Addr::unchecked("bob"),
                    sender: Addr::unchecked("bob"),
                    recipient: Addr::unchecked("alice"),
                },
                coins: Coin {
                    denom: "SECSEC".to_string(),
                    amount: Uint128::from(49u128),
                },
                memo: None,
                block_time: 1571797419,
                block_height: 12345,
            },
            Tx {
                id: 115,
                action: TxAction::Transfer {
                    from: Addr::unchecked("bob"),
                    sender: Addr::unchecked("bob"),
                    recipient: Addr::unchecked("alice"),
                },
                coins: Coin {
                    denom: "SECSEC".to_string(),
                    amount: Uint128::from(48u128),
                },
                memo: None,
                block_time: 1571797419,
                block_height: 12345,
            },
        ];
        assert_eq!(transfers, expected_transfers);

        //
        // check transactions for alice, starting in settled across different bundles with `end` past the last transaction
        // there are 104 transactions total for alice
        // page: 3, page size: 99
        // start is index 99 (100th tx)
        //
        let query_msg = QueryMsg::TransactionHistory {
            address: "alice".to_string(),
            key: "key".to_string(),
            page: Some(3),
            page_size: 33,
            //page: None,
            //page_size: 500,
        };
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        let transfers = match from_binary(&query_result.unwrap()).unwrap() {
            QueryAnswer::TransactionHistory { txs, .. } => txs,
            other => panic!("Unexpected: {:?}", other),
        };
        //println!("transfers: {transfers:?}");
        let expected_transfers = vec![
            Tx {
                id: 69,
                action: TxAction::Transfer {
                    from: Addr::unchecked("bob"),
                    sender: Addr::unchecked("bob"),
                    recipient: Addr::unchecked("alice"),
                },
                coins: Coin {
                    denom: "SECSEC".to_string(),
                    amount: Uint128::from(2u128),
                },
                memo: None,
                block_time: 1571797419,
                block_height: 12345,
            },
            Tx {
                id: 68,
                action: TxAction::Transfer {
                    from: Addr::unchecked("bob"),
                    sender: Addr::unchecked("bob"),
                    recipient: Addr::unchecked("alice"),
                },
                coins: Coin {
                    denom: "SECSEC".to_string(),
                    amount: Uint128::from(1u128),
                },
                memo: None,
                block_time: 1571797419,
                block_height: 12345,
            },
            Tx {
                id: 6,
                action: TxAction::Transfer {
                    from: Addr::unchecked("alice"),
                    sender: Addr::unchecked("alice"),
                    recipient: Addr::unchecked("dora"),
                },
                coins: Coin {
                    denom: "SECSEC".to_string(),
                    amount: Uint128::from(50u128),
                },
                memo: None,
                block_time: 1571797419,
                block_height: 12345,
            },
            Tx {
                id: 4,
                action: TxAction::Transfer {
                    from: Addr::unchecked("bob"),
                    sender: Addr::unchecked("bob"),
                    recipient: Addr::unchecked("alice"),
                },
                coins: Coin {
                    denom: "SECSEC".to_string(),
                    amount: Uint128::from(500u128),
                },
                memo: None,
                block_time: 1571797419,
                block_height: 12345,
            },
            Tx {
                id: 2,
                action: TxAction::Transfer {
                    from: Addr::unchecked("bob"),
                    sender: Addr::unchecked("bob"),
                    recipient: Addr::unchecked("alice"),
                },
                coins: Coin {
                    denom: "SECSEC".to_string(),
                    amount: Uint128::from(1000u128),
                },
                memo: None,
                block_time: 1571797419,
                block_height: 12345,
            },
        ];
        //let transfers_len = transfers.len();
        //println!("transfers.len(): {transfers_len}");
        assert_eq!(transfers, expected_transfers);

        //
        //
        //
        //

        // now try invalid transfer
        let handle_msg = ExecuteMsg::Transfer {
            recipient: "alice".to_string(),
            amount: Uint128::new(10000),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("insufficient funds"));
    }

    #[test]
    fn test_handle_send() {
        let (init_result, mut deps) = init_helper(vec![InitialBalance {
            address: "bob".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let handle_msg = ExecuteMsg::RegisterReceive {
            code_hash: "this_is_a_hash_of_a_code".to_string(),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("contract", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let result = handle_result.unwrap();
        assert!(ensure_success(result));

        let handle_msg = ExecuteMsg::Send {
            recipient: "contract".to_string(),
            recipient_code_hash: None,
            amount: Uint128::new(100),
            memo: Some("my memo".to_string()),
            padding: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            msg: Some(to_binary("hey hey you you").unwrap()),
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let result = handle_result.unwrap();
        assert!(ensure_success(result.clone()));
        let id = 0;
        assert!(result.messages.contains(&SubMsg {
            id,
            msg: CosmosMsg::Wasm(WasmMsg::Execute {
                contract_addr: "contract".to_string(),
                code_hash: "this_is_a_hash_of_a_code".to_string(),
                msg: Snip20ReceiveMsg::new(
                    Addr::unchecked("bob".to_string()),
                    Addr::unchecked("bob".to_string()),
                    Uint128::new(100),
                    Some("my memo".to_string()),
                    Some(to_binary("hey hey you you").unwrap())
                )
                .into_binary()
                .unwrap(),
                funds: vec![],
            })
            .into(),
            reply_on: match id {
                0 => ReplyOn::Never,
                _ => ReplyOn::Always,
            },
            gas_limit: None,
        }));
    }

    #[test]
    fn test_handle_register_receive() {
        let (init_result, mut deps) = init_helper(vec![InitialBalance {
            address: "bob".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let handle_msg = ExecuteMsg::RegisterReceive {
            code_hash: "this_is_a_hash_of_a_code".to_string(),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("contract", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let result = handle_result.unwrap();
        assert!(ensure_success(result));

        let hash =
            legacy_state::get_receiver_hash(&deps.storage, &Addr::unchecked("contract".to_string()))
                .unwrap()
                .unwrap();
        assert_eq!(hash, "this_is_a_hash_of_a_code".to_string());
    }

    #[test]
    fn test_handle_create_viewing_key() {
        let (init_result, mut deps) = init_helper(vec![InitialBalance {
            address: "bob".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let handle_msg = ExecuteMsg::CreateViewingKey {
            entropy: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );
        let answer: ExecuteAnswer = from_binary(&handle_result.unwrap().data.unwrap()).unwrap();

        let key = match answer {
            ExecuteAnswer::CreateViewingKey { key } => key,
            _ => panic!("NOPE"),
        };
        // let bob_canonical = deps.as_mut().api.addr_canonicalize("bob").unwrap();

        let result = ViewingKey::check(&deps.storage, "bob", key.as_str());
        assert!(result.is_ok());

        // let saved_vk = read_viewing_key(&deps.storage, &bob_canonical).unwrap();
        // assert!(key.check_viewing_key(saved_vk.as_slice()));
    }

    #[test]
    fn test_handle_set_viewing_key() {
        let (init_result, mut deps) = init_helper(vec![InitialBalance {
            address: "bob".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        // Set VK
        let handle_msg = ExecuteMsg::SetViewingKey {
            key: "hi lol".to_string(),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let unwrapped_result: ExecuteAnswer =
            from_binary(&handle_result.unwrap().data.unwrap()).unwrap();
        assert_eq!(
            to_binary(&unwrapped_result).unwrap(),
            to_binary(&ExecuteAnswer::SetViewingKey {
                status: ResponseStatus::Success
            })
            .unwrap(),
        );

        // Set valid VK
        let actual_vk = "x".to_string().repeat(VIEWING_KEY_SIZE);
        let handle_msg = ExecuteMsg::SetViewingKey {
            key: actual_vk.clone(),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let unwrapped_result: ExecuteAnswer =
            from_binary(&handle_result.unwrap().data.unwrap()).unwrap();
        assert_eq!(
            to_binary(&unwrapped_result).unwrap(),
            to_binary(&ExecuteAnswer::SetViewingKey { status: Success }).unwrap(),
        );

        let result = ViewingKey::check(&deps.storage, "bob", actual_vk.as_str());
        assert!(result.is_ok());
    }

    fn revoke_permit(
        permit_name: &str,
        user_address: &str,
        deps: &mut OwnedDeps<cosmwasm_std::MemoryStorage, MockApi, MockQuerier>,
    ) -> Result<Response, StdError> {
        let handle_msg = ExecuteMsg::RevokePermit {
            permit_name: permit_name.to_string(),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info(user_address, &[]);
        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);
        handle_result
    }

    fn get_balance_with_permit_qry_msg(
        permit_name: &str,
        chain_id: &str,
        pub_key_value: &str,
        signature: &str,
    ) -> QueryMsg {
        let permit = gen_permit_obj(
            permit_name,
            chain_id,
            pub_key_value,
            signature,
            TokenPermissions::Balance,
        );

        QueryMsg::WithPermit {
            permit,
            query: QueryWithPermit::Balance {},
        }
    }

    fn gen_permit_obj(
        permit_name: &str,
        chain_id: &str,
        pub_key_value: &str,
        signature: &str,
        permit_type: TokenPermissions,
    ) -> Permit {
        let permit: Permit = Permit {
            params: PermitParams {
                allowed_tokens: vec![MOCK_CONTRACT_ADDR.to_string()],
                permit_name: permit_name.to_string(),
                chain_id: chain_id.to_string(),
                permissions: vec![permit_type],
            },
            signature: PermitSignature {
                pub_key: PubKey {
                    r#type: "tendermint/PubKeySecp256k1".to_string(),
                    value: Binary::from_base64(pub_key_value).unwrap(),
                },
                signature: Binary::from_base64(signature).unwrap(),
            },
        };
        permit
    }

    fn get_allowances_given_permit(
        permit_name: &str,
        chain_id: &str,
        pub_key_value: &str,
        signature: &str,
        spender: String,
    ) -> QueryMsg {
        let permit = gen_permit_obj(
            permit_name,
            chain_id,
            pub_key_value,
            signature,
            TokenPermissions::Owner,
        );

        QueryMsg::WithPermit {
            permit,
            query: QueryWithPermit::AllowancesReceived {
                spender,
                page: None,
                page_size: 0,
            },
        }
    }

    #[test]
    fn test_permit_query_allowances_given_should_fail() {
        let user_address = "secret18mdrja40gfuftt5yx6tgj0fn5lurplezyp894y";
        let permit_name = "default";
        let chain_id = "secretdev-1";
        let pub_key = "AkZqxdKMtPq2w0kGDGwWGejTAed0H7azPMHtrCX0XYZG";
        let signature = "ZXyFMlAy6guMG9Gj05rFvcMi5/JGfClRtJpVTHiDtQY3GtSfBHncY70kmYiTXkKIxSxdnh/kS8oXa+GSX5su6Q==";

        // Init the contract
        let (init_result, deps) = init_helper(vec![InitialBalance {
            address: user_address.to_string(),
            amount: Uint128::new(50000000),
        }]);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let msg = get_allowances_given_permit(
            permit_name,
            chain_id,
            pub_key,
            signature,
            "secret1kmgdagt5efcz2kku0ak9ezfgntg29g2vr88q0e".to_string(),
        );
        let query_result = query(deps.as_ref(), mock_env(), msg);

        assert_eq!(query_result.is_err(), true);
    }

    #[test]
    fn test_permit_query_allowances_given() {
        let user_address = "secret18mdrja40gfuftt5yx6tgj0fn5lurplezyp894y";
        let permit_name = "default";
        let chain_id = "secretdev-1";
        let pub_key = "AkZqxdKMtPq2w0kGDGwWGejTAed0H7azPMHtrCX0XYZG";
        let signature = "ZXyFMlAy6guMG9Gj05rFvcMi5/JGfClRtJpVTHiDtQY3GtSfBHncY70kmYiTXkKIxSxdnh/kS8oXa+GSX5su6Q==";

        // Init the contract
        let (init_result, deps) = init_helper(vec![InitialBalance {
            address: user_address.to_string(),
            amount: Uint128::new(50000000),
        }]);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let msg = get_allowances_given_permit(
            permit_name,
            chain_id,
            pub_key,
            signature,
            "secret18mdrja40gfuftt5yx6tgj0fn5lurplezyp894y".to_string(),
        );
        let query_result = query(deps.as_ref(), mock_env(), msg);

        assert_eq!(query_result.is_ok(), true);
    }

    #[test]
    fn test_permit_revoke() {
        let user_address = "secret1kmgdagt5efcz2kku0ak9ezfgntg29g2vr88q0e";
        let permit_name = "to_be_revoked";
        let chain_id = "blabla";

        // Note that 'signature'was generated with the specific values of the above:
        // user_address, permit_name, chain_id, pub_key_value
        let pub_key_value = "Ahlb7vwjo4aTY6dqfgpPmPYF7XhTAIReVwncQwlq8Sct";
        let signature = "VS13F7iv1qxKABxrCAvZQPy2IruLQsIyfTewy/PIhNtybtq417lr3FxsWjV/i9YTqCUxg7weoZwHmYs0YgYX4w==";

        // Init the contract
        let (init_result, mut deps) = init_helper(vec![InitialBalance {
            address: user_address.to_string(),
            amount: Uint128::new(50000000),
        }]);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        // Query the account's balance
        let balance_with_permit_msg =
            get_balance_with_permit_qry_msg(permit_name, chain_id, pub_key_value, signature);
        let query_result = query(deps.as_ref(), mock_env(), balance_with_permit_msg);
        let balance = match from_binary(&query_result.unwrap()).unwrap() {
            QueryAnswer::Balance { amount } => amount,
            _ => panic!("Unexpected result from query"),
        };
        assert_eq!(balance.u128(), 50000000);

        // Revoke the Balance permit
        let handle_result = revoke_permit(permit_name, user_address, &mut deps);
        let status = match from_binary(&handle_result.unwrap().data.unwrap()).unwrap() {
            ExecuteAnswer::RevokePermit { status } => status,
            _ => panic!("NOPE"),
        };
        assert_eq!(status, ResponseStatus::Success);

        // Try to query the balance with permit and fail because the permit is now revoked
        let balance_with_permit_msg =
            get_balance_with_permit_qry_msg(permit_name, chain_id, pub_key_value, signature);
        let query_result = query(deps.as_ref(), mock_env(), balance_with_permit_msg);
        let error = extract_error_msg(query_result);
        assert!(
            error.contains(format!("Permit \"{}\" was revoked by account", permit_name).as_str())
        );
    }

    #[test]
    fn test_execute_transfer_from() {
        let (init_result, mut deps) = init_helper(vec![InitialBalance {
            address: "bob".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        // Transfer before allowance
        let handle_msg = ExecuteMsg::TransferFrom {
            owner: "bob".to_string(),
            recipient: "alice".to_string(),
            amount: Uint128::new(2500),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("alice", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("insufficient allowance"));

        // Transfer more than allowance
        let handle_msg = ExecuteMsg::IncreaseAllowance {
            spender: "alice".to_string(),
            amount: Uint128::new(2000),
            padding: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            expiration: Some(1_571_797_420),
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );
        let handle_msg = ExecuteMsg::TransferFrom {
            owner: "bob".to_string(),
            recipient: "alice".to_string(),
            amount: Uint128::new(2500),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("alice", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("insufficient allowance"));

        // Transfer after allowance expired
        let handle_msg = ExecuteMsg::TransferFrom {
            owner: "bob".to_string(),
            recipient: "alice".to_string(),
            amount: Uint128::new(2000),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };

        let info = MessageInfo {
            sender: Addr::unchecked("bob".to_string()),
            funds: vec![],
        };

        let handle_result = execute(
            deps.as_mut(),
            Env {
                block: BlockInfo {
                    height: 12_345,
                    time: Timestamp::from_seconds(1_571_797_420),
                    chain_id: "cosmos-testnet-14002".to_string(),
                    random: Some(Binary::from(&[
                        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
                        21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
                    ])),
                },
                transaction: Some(TransactionInfo {
                    index: 3,
                    hash: "1010".to_string(),
                }),
                contract: ContractInfo {
                    address: Addr::unchecked(MOCK_CONTRACT_ADDR.to_string()),
                    code_hash: "".to_string(),
                },
            },
            info,
            handle_msg,
        );
        let error = extract_error_msg(handle_result);
        assert!(error.contains("insufficient allowance"));

        // Sanity check
        let handle_msg = ExecuteMsg::TransferFrom {
            owner: "bob".to_string(),
            recipient: "alice".to_string(),
            amount: Uint128::new(2000),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("alice", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );
        let bob_canonical = deps
            .api
            .addr_canonicalize(Addr::unchecked("bob".to_string()).as_str())
            .unwrap();
        let alice_canonical = deps
            .api
            .addr_canonicalize(Addr::unchecked("alice".to_string()).as_str())
            .unwrap();

        let bob_balance = stored_balance(&deps.storage, &bob_canonical).unwrap().unwrap_or_default();
        let alice_balance = stored_balance(&deps.storage, &alice_canonical).unwrap().unwrap_or_default();
        assert_eq!(bob_balance, 5000 - 2000);
        assert_ne!(alice_balance, 2000);
        let total_supply = TOTAL_SUPPLY.load(&deps.storage).unwrap();
        assert_eq!(total_supply, 5000);

        // Second send more than allowance
        let handle_msg = ExecuteMsg::TransferFrom {
            owner: "bob".to_string(),
            recipient: "alice".to_string(),
            amount: Uint128::new(1),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("alice", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("insufficient allowance"));
    }

    #[test]
    fn test_handle_send_from() {
        let (init_result, mut deps) = init_helper(vec![InitialBalance {
            address: "bob".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        // Send before allowance
        let handle_msg = ExecuteMsg::SendFrom {
            owner: "bob".to_string(),
            recipient: "alice".to_string(),
            recipient_code_hash: None,
            amount: Uint128::new(2500),
            memo: None,
            msg: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("alice", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("insufficient allowance"));

        // Send more than allowance
        let handle_msg = ExecuteMsg::IncreaseAllowance {
            spender: "alice".to_string(),
            amount: Uint128::new(2000),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
            expiration: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );
        let handle_msg = ExecuteMsg::SendFrom {
            owner: "bob".to_string(),
            recipient: "alice".to_string(),
            recipient_code_hash: None,
            amount: Uint128::new(2500),
            memo: None,
            msg: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("alice", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("insufficient allowance"));

        // Sanity check
        let handle_msg = ExecuteMsg::RegisterReceive {
            code_hash: "lolz".to_string(),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("contract", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );
        let send_msg = Binary::from(r#"{ "some_msg": { "some_key": "some_val" } }"#.as_bytes());
        let snip20_msg = Snip20ReceiveMsg::new(
            Addr::unchecked("alice".to_string()),
            Addr::unchecked("bob".to_string()),
            Uint128::new(2000),
            Some("my memo".to_string()),
            Some(send_msg.clone()),
        );
        let handle_msg = ExecuteMsg::SendFrom {
            owner: "bob".to_string(),
            recipient: "contract".to_string(),
            recipient_code_hash: None,
            amount: Uint128::new(2000),
            memo: Some("my memo".to_string()),
            msg: Some(send_msg),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("alice", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );
        assert!(handle_result.unwrap().messages.contains(
            &into_cosmos_submsg(
                snip20_msg,
                "lolz".to_string(),
                Addr::unchecked("contract".to_string()),
                0
            )
            .unwrap()
        ));

        let bob_canonical = deps
            .api
            .addr_canonicalize(Addr::unchecked("bob".to_string()).as_str())
            .unwrap();
        let contract_canonical = deps
            .api
            .addr_canonicalize(Addr::unchecked("contract".to_string()).as_str())
            .unwrap();

        let bob_balance = stored_balance(&deps.storage, &bob_canonical).unwrap().unwrap_or_default();
        let contract_balance = stored_balance(&deps.storage, &contract_canonical).unwrap().unwrap_or_default();
        assert_eq!(bob_balance, 5000 - 2000);
        assert_ne!(contract_balance, 2000);
        let total_supply = TOTAL_SUPPLY.load(&deps.storage).unwrap();
        assert_eq!(total_supply, 5000);

        // Second send more than allowance
        let handle_msg = ExecuteMsg::SendFrom {
            owner: "bob".to_string(),
            recipient: "alice".to_string(),
            recipient_code_hash: None,
            amount: Uint128::new(1),
            memo: None,
            msg: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("alice", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("insufficient allowance"));
    }

    #[test]
    fn test_handle_burn_from() {
        let (init_result, mut deps) = init_helper_with_config(
            vec![InitialBalance {
                address: "bob".to_string(),
                amount: Uint128::new(10000),
            }],
            false,
            false,
            false,
            true,
            0,
            vec![],
        );
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let (init_result_for_failure, mut deps_for_failure) = init_helper(vec![InitialBalance {
            address: "bob".to_string(),
            amount: Uint128::new(10000),
        }]);
        assert!(
            init_result_for_failure.is_ok(),
            "Init failed: {}",
            init_result_for_failure.err().unwrap()
        );
        // test when burn disabled
        let handle_msg = ExecuteMsg::BurnFrom {
            owner: "bob".to_string(),
            amount: Uint128::new(2500),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("alice", &[]);

        let handle_result = execute(deps_for_failure.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("Burn functionality is not enabled for this token."));

        // Burn before allowance
        let handle_msg = ExecuteMsg::BurnFrom {
            owner: "bob".to_string(),
            amount: Uint128::new(2500),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("alice", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("insufficient allowance"));

        // Burn more than allowance
        let handle_msg = ExecuteMsg::IncreaseAllowance {
            spender: "alice".to_string(),
            amount: Uint128::new(2000),
            padding: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            expiration: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );
        let handle_msg = ExecuteMsg::BurnFrom {
            owner: "bob".to_string(),
            amount: Uint128::new(2500),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("alice", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("insufficient allowance"));

        // Sanity check
        let handle_msg = ExecuteMsg::BurnFrom {
            owner: "bob".to_string(),
            amount: Uint128::new(2000),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("alice", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );
        let bob_canonical = deps
            .api
            .addr_canonicalize(Addr::unchecked("bob".to_string()).as_str())
            .unwrap();

        let bob_balance = stored_balance(&deps.storage, &bob_canonical).unwrap().unwrap_or_default();
        assert_eq!(bob_balance, 10000 - 2000);
        let total_supply = TOTAL_SUPPLY.load(&deps.storage).unwrap();
        assert_eq!(total_supply, 10000 - 2000);

        // Second burn more than allowance
        let handle_msg = ExecuteMsg::BurnFrom {
            owner: "bob".to_string(),
            amount: Uint128::new(1),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("alice", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("insufficient allowance"));
    }

    #[test]
    fn test_handle_batch_burn_from() {
        let (init_result, mut deps) = init_helper_with_config(
            vec![
                InitialBalance {
                    address: "bob".to_string(),
                    amount: Uint128::new(10000),
                },
                InitialBalance {
                    address: "jerry".to_string(),
                    amount: Uint128::new(10000),
                },
                InitialBalance {
                    address: "mike".to_string(),
                    amount: Uint128::new(10000),
                },
            ],
            false,
            false,
            false,
            true,
            0,
            vec![],
        );
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let (init_result_for_failure, mut deps_for_failure) = init_helper(vec![InitialBalance {
            address: "bob".to_string(),
            amount: Uint128::new(10000),
        }]);
        assert!(
            init_result_for_failure.is_ok(),
            "Init failed: {}",
            init_result_for_failure.err().unwrap()
        );
        // test when burn disabled
        let actions: Vec<_> = ["bob", "jerry", "mike"]
            .iter()
            .map(|name| batch::BurnFromAction {
                owner: name.to_string(),
                amount: Uint128::new(2500),
                memo: None,
            })
            .collect();
        let handle_msg = ExecuteMsg::BatchBurnFrom {
            actions,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("alice", &[]);
        let handle_result = execute(
            deps_for_failure.as_mut(),
            mock_env(),
            info,
            handle_msg.clone(),
        );
        let error = extract_error_msg(handle_result);
        assert!(error.contains("Burn functionality is not enabled for this token."));

        // Burn before allowance
        let info = mock_info("alice", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("insufficient allowance"));

        // Burn more than allowance
        let allowance_size = 2000;
        for name in &["bob", "jerry", "mike"] {
            let handle_msg = ExecuteMsg::IncreaseAllowance {
                spender: "alice".to_string(),
                amount: Uint128::new(allowance_size),
                padding: None,
                #[cfg(feature = "gas_evaporation")]
                gas_target: None,
                expiration: None,
            };
            let info = mock_info(*name, &[]);
            let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

            assert!(
                handle_result.is_ok(),
                "handle() failed: {}",
                handle_result.err().unwrap()
            );
            let handle_msg = ExecuteMsg::BurnFrom {
                owner: "name".to_string(),
                amount: Uint128::new(2500),
                memo: None,
                #[cfg(feature = "gas_evaporation")]
                gas_target: None,
                padding: None,
            };
            let info = mock_info("alice", &[]);

            let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

            let error = extract_error_msg(handle_result);
            assert!(error.contains("insufficient allowance"));
        }

        // Burn some of the allowance
        let actions: Vec<_> = [("bob", 200_u128), ("jerry", 300), ("mike", 400)]
            .iter()
            .map(|(name, amount)| batch::BurnFromAction {
                owner: name.to_string(),
                amount: Uint128::new(*amount),
                memo: None,
            })
            .collect();

        let handle_msg = ExecuteMsg::BatchBurnFrom {
            actions,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("alice", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );
        for (name, amount) in &[("bob", 200_u128), ("jerry", 300), ("mike", 400)] {
            let name_canon = deps
                .api
                .addr_canonicalize(Addr::unchecked(name.to_string()).as_str())
                .unwrap();
            let balance = stored_balance(&deps.storage, &name_canon).unwrap().unwrap_or_default();
            assert_eq!(balance, 10000 - amount);
        }
        let total_supply = TOTAL_SUPPLY.load(&deps.storage).unwrap();
        assert_eq!(total_supply, 10000 * 3 - (200 + 300 + 400));

        // Burn the rest of the allowance
        let actions: Vec<_> = [("bob", 200_u128), ("jerry", 300), ("mike", 400)]
            .iter()
            .map(|(name, amount)| batch::BurnFromAction {
                owner: name.to_string(),
                amount: Uint128::new(allowance_size - *amount),
                memo: None,
            })
            .collect();

        let handle_msg = ExecuteMsg::BatchBurnFrom {
            actions,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("alice", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );
        for name in &["bob", "jerry", "mike"] {
            let name_canon = deps
                .api
                .addr_canonicalize(Addr::unchecked(name.to_string()).as_str())
                .unwrap();
            let balance = stored_balance(&deps.storage, &name_canon).unwrap().unwrap_or_default();
            assert_eq!(balance, 10000 - allowance_size);
        }
        let total_supply = TOTAL_SUPPLY.load(&deps.storage).unwrap();
        assert_eq!(total_supply, 3 * (10000 - allowance_size));

        // Second burn more than allowance
        let actions: Vec<_> = ["bob", "jerry", "mike"]
            .iter()
            .map(|name| batch::BurnFromAction {
                owner: name.to_string(),
                amount: Uint128::new(1),
                memo: None,
            })
            .collect();
        let handle_msg = ExecuteMsg::BatchBurnFrom {
            actions,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("alice", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("insufficient allowance"));
    }

    #[test]
    fn test_handle_decrease_allowance() {
        let (init_result, mut deps) = init_helper(vec![InitialBalance {
            address: "bob".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let handle_msg = ExecuteMsg::DecreaseAllowance {
            spender: "alice".to_string(),
            amount: Uint128::new(2000),
            padding: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            expiration: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );

        let bob_canonical = Addr::unchecked("bob".to_string());
        let alice_canonical = Addr::unchecked("alice".to_string());

        let allowance = AllowancesStore::load(&deps.storage, &bob_canonical, &alice_canonical);
        assert_eq!(
            allowance,
            crate::state::Allowance {
                amount: 0,
                expiration: None
            }
        );

        let handle_msg = ExecuteMsg::IncreaseAllowance {
            spender: "alice".to_string(),
            amount: Uint128::new(2000),
            padding: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            expiration: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );

        let handle_msg = ExecuteMsg::DecreaseAllowance {
            spender: "alice".to_string(),
            amount: Uint128::new(50),
            padding: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            expiration: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );

        let allowance = AllowancesStore::load(&deps.storage, &bob_canonical, &alice_canonical);
        assert_eq!(
            allowance,
            crate::state::Allowance {
                amount: 1950,
                expiration: None
            }
        );
    }

    #[test]
    fn test_handle_increase_allowance() {
        let (init_result, mut deps) = init_helper(vec![InitialBalance {
            address: "bob".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let handle_msg = ExecuteMsg::IncreaseAllowance {
            spender: "alice".to_string(),
            amount: Uint128::new(2000),
            padding: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            expiration: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );

        let bob_canonical = Addr::unchecked("bob".to_string());
        let alice_canonical = Addr::unchecked("alice".to_string());

        let allowance = AllowancesStore::load(&deps.storage, &bob_canonical, &alice_canonical);
        assert_eq!(
            allowance,
            crate::state::Allowance {
                amount: 2000,
                expiration: None
            }
        );

        let handle_msg = ExecuteMsg::IncreaseAllowance {
            spender: "alice".to_string(),
            amount: Uint128::new(2000),
            padding: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            expiration: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );

        let allowance = AllowancesStore::load(&deps.storage, &bob_canonical, &alice_canonical);
        assert_eq!(
            allowance,
            crate::state::Allowance {
                amount: 4000,
                expiration: None
            }
        );
    }

    #[test]
    fn test_handle_change_admin() {
        let (init_result, mut deps) = init_helper(vec![InitialBalance {
            address: "bob".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let handle_msg = ExecuteMsg::ChangeAdmin {
            address: "bob".to_string(),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("admin", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );

        let admin = CONFIG.load(&deps.storage).unwrap().admin;
        assert_eq!(admin, Addr::unchecked("bob".to_string()));
    }

    #[test]
    fn test_handle_set_contract_status() {
        let (init_result, mut deps) = init_helper(vec![InitialBalance {
            address: "admin".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let handle_msg = ExecuteMsg::SetContractStatus {
            level: ContractStatusLevel::StopAll,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("admin", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );

        let contract_status = CONTRACT_STATUS.load(&deps.storage).unwrap();
        assert!(matches!(
            contract_status,
            ContractStatusLevel::StopAll { .. }
        ));
    }

    #[test]
    fn test_handle_redeem() {
        let (init_result, mut deps) = init_helper_with_config(
            vec![InitialBalance {
                address: "butler".to_string(),
                amount: Uint128::new(5000),
            }],
            false,
            true,
            false,
            false,
            1000,
            vec!["uscrt".to_string()],
        );
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let (init_result_no_reserve, mut deps_no_reserve) = init_helper_with_config(
            vec![InitialBalance {
                address: "butler".to_string(),
                amount: Uint128::new(5000),
            }],
            false,
            true,
            false,
            false,
            0,
            vec!["uscrt".to_string()],
        );
        assert!(
            init_result_no_reserve.is_ok(),
            "Init failed: {}",
            init_result_no_reserve.err().unwrap()
        );

        let (init_result_for_failure, mut deps_for_failure) = init_helper(vec![InitialBalance {
            address: "butler".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result_for_failure.is_ok(),
            "Init failed: {}",
            init_result_for_failure.err().unwrap()
        );
        // test when redeem disabled
        let handle_msg = ExecuteMsg::Redeem {
            amount: Uint128::new(1000),
            denom: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("butler", &[]);

        let handle_result = execute(deps_for_failure.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("Redeem functionality is not enabled for this token."));

        // try to redeem when contract has 0 balance
        let handle_msg = ExecuteMsg::Redeem {
            amount: Uint128::new(1000),
            denom: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("butler", &[]);

        let handle_result = execute(deps_no_reserve.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert_eq!(
            error,
            "You are trying to redeem for more uscrt than the contract has in its reserve"
        );

        // test without denom
        let handle_msg = ExecuteMsg::Redeem {
            amount: Uint128::new(1000),
            denom: None,
            padding: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
        };
        let info = mock_info("butler", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );

        // test with denom specified
        let handle_msg = ExecuteMsg::Redeem {
            amount: Uint128::new(1000),
            denom: Option::from("uscrt".to_string()),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("butler", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );

        let canonical = deps
            .api
            .addr_canonicalize(Addr::unchecked("butler".to_string()).as_str())
            .unwrap();
        assert_eq!(stored_balance(&deps.storage, &canonical).unwrap().unwrap_or_default(), 3000)
    }

    #[test]
    fn test_handle_deposit() {
        let (init_result, mut deps) = init_helper_with_config(
            vec![InitialBalance {
                address: "lebron".to_string(),
                amount: Uint128::new(5000),
            }],
            true,
            false,
            false,
            false,
            0,
            vec!["uscrt".to_string()],
        );
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let (init_result_for_failure, mut deps_for_failure) = init_helper(vec![InitialBalance {
            address: "lebron".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result_for_failure.is_ok(),
            "Init failed: {}",
            init_result_for_failure.err().unwrap()
        );
        // test when deposit disabled
        let handle_msg = ExecuteMsg::Deposit {
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info(
            "lebron",
            &[Coin {
                denom: "uscrt".to_string(),
                amount: Uint128::new(1000),
            }],
        );

        let handle_result = execute(deps_for_failure.as_mut(), mock_env(), info, handle_msg);
        let error = extract_error_msg(handle_result);
        assert!(error.contains("Tried to deposit an unsupported coin uscrt"));

        let handle_msg = ExecuteMsg::Deposit {
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };

        let info = mock_info(
            "lebron",
            &[Coin {
                denom: "uscrt".to_string(),
                amount: Uint128::new(1000),
            }],
        );

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);
        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );

        let canonical = deps
            .api
            .addr_canonicalize(Addr::unchecked("lebron".to_string()).as_str())
            .unwrap();

        // stored balance not updated, still in dwb
        assert_ne!(stored_balance(&deps.storage, &canonical).unwrap().unwrap_or_default(), 6000);

        let create_vk_msg = ExecuteMsg::CreateViewingKey {
            entropy: Some("34".to_string()),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("lebron", &[]);
        let handle_response = execute(deps.as_mut(), mock_env(), info, create_vk_msg).unwrap();
        let vk = match from_binary(&handle_response.data.unwrap()).unwrap() {
            ExecuteAnswer::CreateViewingKey { key } => key,
            _ => panic!("Unexpected result from handle"),
        };

        let query_balance_msg = QueryMsg::Balance {
            address: "lebron".to_string(),
            key: vk,
        };

        let query_response = query(deps.as_ref(), mock_env(), query_balance_msg).unwrap();
        let balance = match from_binary(&query_response).unwrap() {
            QueryAnswer::Balance { amount } => amount,
            _ => panic!("Unexpected result from query"),
        };
        assert_eq!(balance, Uint128::new(6000));
    }

    #[test]
    fn test_handle_burn() {
        let (init_result, mut deps) = init_helper_with_config(
            vec![InitialBalance {
                address: "lebron".to_string(),
                amount: Uint128::new(5000),
            }],
            false,
            false,
            false,
            true,
            0,
            vec![],
        );
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let (init_result_for_failure, mut deps_for_failure) = init_helper(vec![InitialBalance {
            address: "lebron".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result_for_failure.is_ok(),
            "Init failed: {}",
            init_result_for_failure.err().unwrap()
        );
        // test when burn disabled
        let handle_msg = ExecuteMsg::Burn {
            amount: Uint128::new(100),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("lebron", &[]);

        let handle_result = execute(deps_for_failure.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("Burn functionality is not enabled for this token."));

        let supply = TOTAL_SUPPLY.load(&deps.storage).unwrap();
        let burn_amount: u128 = 100;
        let handle_msg = ExecuteMsg::Burn {
            amount: Uint128::new(burn_amount),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("lebron", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "Pause handle failed: {}",
            handle_result.err().unwrap()
        );

        let new_supply = TOTAL_SUPPLY.load(&deps.storage).unwrap();
        assert_eq!(new_supply, supply - burn_amount);
    }

    #[test]
    fn test_handle_mint() {
        let (init_result, mut deps) = init_helper_with_config(
            vec![InitialBalance {
                address: "lebron".to_string(),
                amount: Uint128::new(5000),
            }],
            false,
            false,
            true,
            false,
            0,
            vec![],
        );
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );
        let (init_result_for_failure, mut deps_for_failure) = init_helper(vec![InitialBalance {
            address: "lebron".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result_for_failure.is_ok(),
            "Init failed: {}",
            init_result_for_failure.err().unwrap()
        );
        // try to mint when mint is disabled
        let mint_amount: u128 = 100;
        let handle_msg = ExecuteMsg::Mint {
            recipient: "lebron".to_string(),
            amount: Uint128::new(mint_amount),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("admin", &[]);

        let handle_result = execute(deps_for_failure.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("Mint functionality is not enabled for this token"));

        let supply = TOTAL_SUPPLY.load(&deps.storage).unwrap();
        let mint_amount: u128 = 100;
        let handle_msg = ExecuteMsg::Mint {
            recipient: "lebron".to_string(),
            amount: Uint128::new(mint_amount),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("admin", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "Pause handle failed: {}",
            handle_result.err().unwrap()
        );

        let new_supply = TOTAL_SUPPLY.load(&deps.storage).unwrap();
        assert_eq!(new_supply, supply + mint_amount);
    }

    #[test]
    fn test_handle_admin_commands() {
        let admin_err = "Admin commands can only be run from admin address".to_string();
        let (init_result, mut deps) = init_helper_with_config(
            vec![InitialBalance {
                address: "lebron".to_string(),
                amount: Uint128::new(5000),
            }],
            false,
            false,
            true,
            false,
            0,
            vec![],
        );
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let pause_msg = ExecuteMsg::SetContractStatus {
            level: ContractStatusLevel::StopAllButRedeems,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("not_admin", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, pause_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains(&admin_err.clone()));

        let mint_msg = ExecuteMsg::AddMinters {
            minters: vec!["not_admin".to_string()],
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("not_admin", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, mint_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains(&admin_err.clone()));

        let mint_msg = ExecuteMsg::RemoveMinters {
            minters: vec!["admin".to_string()],
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("not_admin", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, mint_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains(&admin_err.clone()));

        let mint_msg = ExecuteMsg::SetMinters {
            minters: vec!["not_admin".to_string()],
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("not_admin", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, mint_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains(&admin_err.clone()));

        let change_admin_msg = ExecuteMsg::ChangeAdmin {
            address: "not_admin".to_string(),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("not_admin", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, change_admin_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains(&admin_err.clone()));
    }

    #[test]
    fn test_handle_pause_with_withdrawals() {
        let (init_result, mut deps) = init_helper_with_config(
            vec![InitialBalance {
                address: "lebron".to_string(),
                amount: Uint128::new(5000),
            }],
            false,
            true,
            false,
            false,
            5000,
            vec!["uscrt".to_string()],
        );
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let pause_msg = ExecuteMsg::SetContractStatus {
            level: ContractStatusLevel::StopAllButRedeems,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };

        let info = mock_info("admin", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, pause_msg);

        assert!(
            handle_result.is_ok(),
            "Pause handle failed: {}",
            handle_result.err().unwrap()
        );

        let send_msg = ExecuteMsg::Transfer {
            recipient: "account".to_string(),
            amount: Uint128::new(123),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("admin", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, send_msg);

        let error = extract_error_msg(handle_result);
        assert_eq!(
            error,
            "This contract is stopped and this action is not allowed".to_string()
        );

        let withdraw_msg = ExecuteMsg::Redeem {
            amount: Uint128::new(5000),
            denom: Option::from("uscrt".to_string()),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("lebron", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, withdraw_msg);

        assert!(
            handle_result.is_ok(),
            "Withdraw failed: {}",
            handle_result.err().unwrap()
        );
    }

    #[test]
    fn test_handle_pause_all() {
        let (init_result, mut deps) = init_helper(vec![InitialBalance {
            address: "lebron".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let pause_msg = ExecuteMsg::SetContractStatus {
            level: ContractStatusLevel::StopAll,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };

        let info = mock_info("admin", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, pause_msg);

        assert!(
            handle_result.is_ok(),
            "Pause handle failed: {}",
            handle_result.err().unwrap()
        );

        let send_msg = ExecuteMsg::Transfer {
            recipient: "account".to_string(),
            amount: Uint128::new(123),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("admin", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, send_msg);

        let error = extract_error_msg(handle_result);
        assert_eq!(
            error,
            "This contract is stopped and this action is not allowed".to_string()
        );

        let withdraw_msg = ExecuteMsg::Redeem {
            amount: Uint128::new(5000),
            denom: Option::from("uscrt".to_string()),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("lebron", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, withdraw_msg);

        let error = extract_error_msg(handle_result);
        assert_eq!(
            error,
            "This contract is stopped and this action is not allowed".to_string()
        );
    }

    #[test]
    fn test_handle_set_minters() {
        let (init_result, mut deps) = init_helper_with_config(
            vec![InitialBalance {
                address: "bob".to_string(),
                amount: Uint128::new(5000),
            }],
            false,
            false,
            true,
            false,
            0,
            vec![],
        );
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );
        let (init_result_for_failure, mut deps_for_failure) = init_helper(vec![InitialBalance {
            address: "bob".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result_for_failure.is_ok(),
            "Init failed: {}",
            init_result_for_failure.err().unwrap()
        );
        // try when mint disabled
        let handle_msg = ExecuteMsg::SetMinters {
            minters: vec!["bob".to_string()],
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("admin", &[]);

        let handle_result = execute(deps_for_failure.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("Mint functionality is not enabled for this token"));

        let handle_msg = ExecuteMsg::SetMinters {
            minters: vec!["bob".to_string()],
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("Admin commands can only be run from admin address"));

        let handle_msg = ExecuteMsg::SetMinters {
            minters: vec!["bob".to_string()],
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("admin", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(ensure_success(handle_result.unwrap()));

        let handle_msg = ExecuteMsg::Mint {
            recipient: "bob".to_string(),
            amount: Uint128::new(100),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(ensure_success(handle_result.unwrap()));

        let handle_msg = ExecuteMsg::Mint {
            recipient: "bob".to_string(),
            amount: Uint128::new(100),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("admin", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("allowed to minter accounts only"));
    }

    #[test]
    fn test_handle_add_minters() {
        let (init_result, mut deps) = init_helper_with_config(
            vec![InitialBalance {
                address: "bob".to_string(),
                amount: Uint128::new(5000),
            }],
            false,
            false,
            true,
            false,
            0,
            vec![],
        );
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );
        let (init_result_for_failure, mut deps_for_failure) = init_helper(vec![InitialBalance {
            address: "bob".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result_for_failure.is_ok(),
            "Init failed: {}",
            init_result_for_failure.err().unwrap()
        );
        // try when mint disabled
        let handle_msg = ExecuteMsg::AddMinters {
            minters: vec!["bob".to_string()],
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("admin", &[]);

        let handle_result = execute(deps_for_failure.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("Mint functionality is not enabled for this token"));

        let handle_msg = ExecuteMsg::AddMinters {
            minters: vec!["bob".to_string()],
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("Admin commands can only be run from admin address"));

        let handle_msg = ExecuteMsg::AddMinters {
            minters: vec!["bob".to_string()],
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("admin", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(ensure_success(handle_result.unwrap()));

        let handle_msg = ExecuteMsg::Mint {
            recipient: "bob".to_string(),
            amount: Uint128::new(100),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(ensure_success(handle_result.unwrap()));

        let handle_msg = ExecuteMsg::Mint {
            recipient: "bob".to_string(),
            amount: Uint128::new(100),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("admin", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(ensure_success(handle_result.unwrap()));
    }

    #[test]
    fn test_handle_remove_minters() {
        let (init_result, mut deps) = init_helper_with_config(
            vec![InitialBalance {
                address: "bob".to_string(),
                amount: Uint128::new(5000),
            }],
            false,
            false,
            true,
            false,
            0,
            vec![],
        );
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );
        let (init_result_for_failure, mut deps_for_failure) = init_helper(vec![InitialBalance {
            address: "bob".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result_for_failure.is_ok(),
            "Init failed: {}",
            init_result_for_failure.err().unwrap()
        );
        // try when mint disabled
        let handle_msg = ExecuteMsg::RemoveMinters {
            minters: vec!["bob".to_string()],
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("admin", &[]);

        let handle_result = execute(deps_for_failure.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("Mint functionality is not enabled for this token"));

        let handle_msg = ExecuteMsg::RemoveMinters {
            minters: vec!["admin".to_string()],
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("Admin commands can only be run from admin address"));

        let handle_msg = ExecuteMsg::RemoveMinters {
            minters: vec!["admin".to_string()],
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("admin", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(ensure_success(handle_result.unwrap()));

        let handle_msg = ExecuteMsg::Mint {
            recipient: "bob".to_string(),
            amount: Uint128::new(100),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("allowed to minter accounts only"));

        let handle_msg = ExecuteMsg::Mint {
            recipient: "bob".to_string(),
            amount: Uint128::new(100),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("admin", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("allowed to minter accounts only"));

        // Removing another extra time to ensure nothing funky happens
        let handle_msg = ExecuteMsg::RemoveMinters {
            minters: vec!["admin".to_string()],
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("admin", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(ensure_success(handle_result.unwrap()));

        let handle_msg = ExecuteMsg::Mint {
            recipient: "bob".to_string(),
            amount: Uint128::new(100),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("allowed to minter accounts only"));

        let handle_msg = ExecuteMsg::Mint {
            recipient: "bob".to_string(),
            amount: Uint128::new(100),
            memo: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("admin", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let error = extract_error_msg(handle_result);
        assert!(error.contains("allowed to minter accounts only"));
    }

    // Query tests

    #[test]
    fn test_authenticated_queries() {
        let (init_result, mut deps) = init_helper(vec![InitialBalance {
            address: "giannis".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let no_vk_yet_query_msg = QueryMsg::Balance {
            address: "giannis".to_string(),
            key: "no_vk_yet".to_string(),
        };
        let query_result = query(deps.as_ref(), mock_env(), no_vk_yet_query_msg);
        let error = extract_error_msg(query_result);
        assert_eq!(
            error,
            "Wrong viewing key for this address or viewing key not set".to_string()
        );

        let create_vk_msg = ExecuteMsg::CreateViewingKey {
            entropy: Some("34".to_string()),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("giannis", &[]);
        let handle_response = execute(deps.as_mut(), mock_env(), info, create_vk_msg).unwrap();
        let vk = match from_binary(&handle_response.data.unwrap()).unwrap() {
            ExecuteAnswer::CreateViewingKey { key } => key,
            _ => panic!("Unexpected result from handle"),
        };

        let query_balance_msg = QueryMsg::Balance {
            address: "giannis".to_string(),
            key: vk,
        };

        let query_response = query(deps.as_ref(), mock_env(), query_balance_msg).unwrap();
        let balance = match from_binary(&query_response).unwrap() {
            QueryAnswer::Balance { amount } => amount,
            _ => panic!("Unexpected result from query"),
        };
        assert_eq!(balance, Uint128::new(5000));

        let wrong_vk_query_msg = QueryMsg::Balance {
            address: "giannis".to_string(),
            key: "wrong_vk".to_string(),
        };
        let query_result = query(deps.as_ref(), mock_env(), wrong_vk_query_msg);
        let error = extract_error_msg(query_result);
        assert_eq!(
            error,
            "Wrong viewing key for this address or viewing key not set".to_string()
        );
    }

    #[test]
    fn test_query_token_info() {
        let init_name = "sec-sec".to_string();
        let init_admin = Addr::unchecked("admin".to_string());
        let init_symbol = "SECSEC".to_string();
        let init_decimals = 8;
        let init_config: InitConfig = from_binary(&Binary::from(
            r#"{ "public_total_supply": true }"#.as_bytes(),
        ))
        .unwrap();
        let init_supply = Uint128::new(5000);

        let mut deps = mock_dependencies_with_balance(&[]);
        let info = mock_info("instantiator", &[]);
        let env = mock_env();
        let init_msg = InstantiateMsg {
            name: init_name.clone(),
            admin: Some(init_admin.into_string()),
            symbol: init_symbol.clone(),
            decimals: init_decimals.clone(),
            initial_balances: Some(vec![InitialBalance {
                address: "giannis".to_string(),
                amount: init_supply,
            }]),
            prng_seed: Binary::from("lolz fun yay".as_bytes()),
            config: Some(init_config),
            supported_denoms: None,
        };
        let init_result = instantiate(deps.as_mut(), env, info, init_msg);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let query_msg = QueryMsg::TokenInfo {};
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        assert!(
            query_result.is_ok(),
            "Init failed: {}",
            query_result.err().unwrap()
        );
        let query_answer: QueryAnswer = from_binary(&query_result.unwrap()).unwrap();
        match query_answer {
            QueryAnswer::TokenInfo {
                name,
                symbol,
                decimals,
                total_supply,
            } => {
                assert_eq!(name, init_name);
                assert_eq!(symbol, init_symbol);
                assert_eq!(decimals, init_decimals);
                assert_eq!(total_supply, Some(Uint128::new(5000)));
            }
            _ => panic!("unexpected"),
        }
    }

    #[test]
    fn test_query_token_config() {
        let init_name = "sec-sec".to_string();
        let init_admin = Addr::unchecked("admin".to_string());
        let init_symbol = "SECSEC".to_string();
        let init_decimals = 8;
        let init_config: InitConfig = from_binary(&Binary::from(
            format!(
                "{{\"public_total_supply\":{},
            \"enable_deposit\":{},
            \"enable_redeem\":{},
            \"enable_mint\":{},
            \"enable_burn\":{}}}",
                true, false, false, true, false
            )
            .as_bytes(),
        ))
        .unwrap();

        let init_supply = Uint128::new(5000);

        let mut deps = mock_dependencies_with_balance(&[]);
        let info = mock_info("instantiator", &[]);
        let env = mock_env();
        let init_msg = InstantiateMsg {
            name: init_name.clone(),
            admin: Some(init_admin.into_string()),
            symbol: init_symbol.clone(),
            decimals: init_decimals.clone(),
            initial_balances: Some(vec![InitialBalance {
                address: "giannis".to_string(),
                amount: init_supply,
            }]),
            prng_seed: Binary::from("lolz fun yay".as_bytes()),
            config: Some(init_config),
            supported_denoms: None,
        };
        let init_result = instantiate(deps.as_mut(), env, info, init_msg);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let query_msg = QueryMsg::TokenConfig {};
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        assert!(
            query_result.is_ok(),
            "Init failed: {}",
            query_result.err().unwrap()
        );
        let query_answer: QueryAnswer = from_binary(&query_result.unwrap()).unwrap();
        match query_answer {
            QueryAnswer::TokenConfig {
                public_total_supply,
                deposit_enabled,
                redeem_enabled,
                mint_enabled,
                burn_enabled,
                supported_denoms,
            } => {
                assert_eq!(public_total_supply, true);
                assert_eq!(deposit_enabled, false);
                assert_eq!(redeem_enabled, false);
                assert_eq!(mint_enabled, true);
                assert_eq!(burn_enabled, false);
                assert_eq!(supported_denoms.len(), 0);
            }
            _ => panic!("unexpected"),
        }
    }

    #[test]
    fn test_query_exchange_rate() {
        // test more dec than SCRT
        let init_name = "sec-sec".to_string();
        let init_admin = Addr::unchecked("admin".to_string());
        let init_symbol = "SECSEC".to_string();
        let init_decimals = 8;

        let init_supply = Uint128::new(5000);

        let mut deps = mock_dependencies_with_balance(&[]);
        let info = mock_info("instantiator", &[]);
        let env = mock_env();
        let init_config: InitConfig = from_binary(&Binary::from(
            format!(
                "{{\"public_total_supply\":{},
                \"enable_deposit\":{},
                \"enable_redeem\":{},
                \"enable_mint\":{},
                \"enable_burn\":{}}}",
                true, true, false, false, false
            )
            .as_bytes(),
        ))
        .unwrap();
        let init_msg = InstantiateMsg {
            name: init_name.clone(),
            admin: Some(init_admin.into_string()),
            symbol: init_symbol.clone(),
            decimals: init_decimals.clone(),
            initial_balances: Some(vec![InitialBalance {
                address: "giannis".to_string(),
                amount: init_supply,
            }]),
            prng_seed: Binary::from("lolz fun yay".as_bytes()),
            config: Some(init_config),
            supported_denoms: Some(vec!["uscrt".to_string()]),
        };
        let init_result = instantiate(deps.as_mut(), env, info, init_msg);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let query_msg = QueryMsg::ExchangeRate {};
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        assert!(
            query_result.is_ok(),
            "Init failed: {}",
            query_result.err().unwrap()
        );
        let query_answer: QueryAnswer = from_binary(&query_result.unwrap()).unwrap();
        match query_answer {
            QueryAnswer::ExchangeRate { rate, denom } => {
                assert_eq!(rate, Uint128::new(100));
                assert_eq!(denom, "SCRT");
            }
            _ => panic!("unexpected"),
        }

        // test same number of decimals as SCRT
        let init_name = "sec-sec".to_string();
        let init_admin = Addr::unchecked("admin".to_string());
        let init_symbol = "SECSEC".to_string();
        let init_decimals = 6;

        let init_supply = Uint128::new(5000);

        let mut deps = mock_dependencies_with_balance(&[]);
        let info = mock_info("instantiator", &[]);
        let env = mock_env();
        let init_config: InitConfig = from_binary(&Binary::from(
            format!(
                "{{\"public_total_supply\":{},
            \"enable_deposit\":{},
            \"enable_redeem\":{},
            \"enable_mint\":{},
            \"enable_burn\":{}}}",
                true, true, false, false, false
            )
            .as_bytes(),
        ))
        .unwrap();
        let init_msg = InstantiateMsg {
            name: init_name.clone(),
            admin: Some(init_admin.into_string()),
            symbol: init_symbol.clone(),
            decimals: init_decimals.clone(),
            initial_balances: Some(vec![InitialBalance {
                address: "giannis".to_string(),
                amount: init_supply,
            }]),
            prng_seed: Binary::from("lolz fun yay".as_bytes()),
            config: Some(init_config),
            supported_denoms: Some(vec!["uscrt".to_string()]),
        };
        let init_result = instantiate(deps.as_mut(), env, info, init_msg);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let query_msg = QueryMsg::ExchangeRate {};
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        assert!(
            query_result.is_ok(),
            "Init failed: {}",
            query_result.err().unwrap()
        );
        let query_answer: QueryAnswer = from_binary(&query_result.unwrap()).unwrap();
        match query_answer {
            QueryAnswer::ExchangeRate { rate, denom } => {
                assert_eq!(rate, Uint128::new(1));
                assert_eq!(denom, "SCRT");
            }
            _ => panic!("unexpected"),
        }

        // test less decimal places than SCRT
        let init_name = "sec-sec".to_string();
        let init_admin = Addr::unchecked("admin".to_string());
        let init_symbol = "SECSEC".to_string();
        let init_decimals = 3;

        let init_supply = Uint128::new(5000);

        let mut deps = mock_dependencies_with_balance(&[]);
        let info = mock_info("instantiator", &[]);
        let env = mock_env();
        let init_config: InitConfig = from_binary(&Binary::from(
            format!(
                "{{\"public_total_supply\":{},
            \"enable_deposit\":{},
            \"enable_redeem\":{},
            \"enable_mint\":{},
            \"enable_burn\":{}}}",
                true, true, false, false, false
            )
            .as_bytes(),
        ))
        .unwrap();
        let init_msg = InstantiateMsg {
            name: init_name.clone(),
            admin: Some(init_admin.into_string()),
            symbol: init_symbol.clone(),
            decimals: init_decimals.clone(),
            initial_balances: Some(vec![InitialBalance {
                address: "giannis".to_string(),
                amount: init_supply,
            }]),
            prng_seed: Binary::from("lolz fun yay".as_bytes()),
            config: Some(init_config),
            supported_denoms: Some(vec!["uscrt".to_string()]),
        };
        let init_result = instantiate(deps.as_mut(), env, info, init_msg);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let query_msg = QueryMsg::ExchangeRate {};
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        assert!(
            query_result.is_ok(),
            "Init failed: {}",
            query_result.err().unwrap()
        );
        let query_answer: QueryAnswer = from_binary(&query_result.unwrap()).unwrap();
        match query_answer {
            QueryAnswer::ExchangeRate { rate, denom } => {
                assert_eq!(rate, Uint128::new(1000));
                assert_eq!(denom, "SECSEC");
            }
            _ => panic!("unexpected"),
        }

        // test depost/redeem not enabled
        let init_name = "sec-sec".to_string();
        let init_admin = Addr::unchecked("admin".to_string());
        let init_symbol = "SECSEC".to_string();
        let init_decimals = 3;

        let init_supply = Uint128::new(5000);

        let mut deps = mock_dependencies_with_balance(&[]);
        let info = mock_info("instantiator", &[]);
        let env = mock_env();
        let init_msg = InstantiateMsg {
            name: init_name.clone(),
            admin: Some(init_admin.into_string()),
            symbol: init_symbol.clone(),
            decimals: init_decimals.clone(),
            initial_balances: Some(vec![InitialBalance {
                address: "giannis".to_string(),
                amount: init_supply,
            }]),
            prng_seed: Binary::from("lolz fun yay".as_bytes()),
            config: None,
            supported_denoms: None,
        };
        let init_result = instantiate(deps.as_mut(), env, info, init_msg);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let query_msg = QueryMsg::ExchangeRate {};
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        assert!(
            query_result.is_ok(),
            "Init failed: {}",
            query_result.err().unwrap()
        );
        let query_answer: QueryAnswer = from_binary(&query_result.unwrap()).unwrap();
        match query_answer {
            QueryAnswer::ExchangeRate { rate, denom } => {
                assert_eq!(rate, Uint128::new(0));
                assert_eq!(denom, String::new());
            }
            _ => panic!("unexpected"),
        }
    }

    #[test]
    fn test_query_allowance() {
        let (init_result, mut deps) = init_helper(vec![InitialBalance {
            address: "giannis".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let handle_msg = ExecuteMsg::IncreaseAllowance {
            spender: "lebron".to_string(),
            amount: Uint128::new(2000),
            padding: None,
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            expiration: None,
        };
        let info = mock_info("giannis", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );

        let vk1 = "key1".to_string();
        let vk2 = "key2".to_string();

        let query_msg = QueryMsg::Allowance {
            owner: "giannis".to_string(),
            spender: "lebron".to_string(),
            key: vk1.clone(),
        };
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        assert!(
            query_result.is_ok(),
            "Query failed: {}",
            query_result.err().unwrap()
        );
        let error = extract_error_msg(query_result);
        assert!(error.contains("Wrong viewing key"));

        let handle_msg = ExecuteMsg::SetViewingKey {
            key: vk1.clone(),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("lebron", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let unwrapped_result: ExecuteAnswer =
            from_binary(&handle_result.unwrap().data.unwrap()).unwrap();
        assert_eq!(
            to_binary(&unwrapped_result).unwrap(),
            to_binary(&ExecuteAnswer::SetViewingKey {
                status: ResponseStatus::Success
            })
            .unwrap(),
        );

        let handle_msg = ExecuteMsg::SetViewingKey {
            key: vk2.clone(),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("giannis", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let unwrapped_result: ExecuteAnswer =
            from_binary(&handle_result.unwrap().data.unwrap()).unwrap();
        assert_eq!(
            to_binary(&unwrapped_result).unwrap(),
            to_binary(&ExecuteAnswer::SetViewingKey {
                status: ResponseStatus::Success
            })
            .unwrap(),
        );

        let query_msg = QueryMsg::Allowance {
            owner: "giannis".to_string(),
            spender: "lebron".to_string(),
            key: vk1.clone(),
        };
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        let allowance = match from_binary(&query_result.unwrap()).unwrap() {
            QueryAnswer::Allowance { allowance, .. } => allowance,
            _ => panic!("Unexpected"),
        };
        assert_eq!(allowance, Uint128::new(2000));

        let query_msg = QueryMsg::Allowance {
            owner: "giannis".to_string(),
            spender: "lebron".to_string(),
            key: vk2.clone(),
        };
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        let allowance = match from_binary(&query_result.unwrap()).unwrap() {
            QueryAnswer::Allowance { allowance, .. } => allowance,
            _ => panic!("Unexpected"),
        };
        assert_eq!(allowance, Uint128::new(2000));

        let query_msg = QueryMsg::Allowance {
            owner: "lebron".to_string(),
            spender: "giannis".to_string(),
            key: vk2.clone(),
        };
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        let allowance = match from_binary(&query_result.unwrap()).unwrap() {
            QueryAnswer::Allowance { allowance, .. } => allowance,
            _ => panic!("Unexpected"),
        };
        assert_eq!(allowance, Uint128::new(0));
    }

    #[test]
    fn test_query_all_allowances() {
        let num_owners = 3;
        let num_spenders = 20;
        let vk = "key".to_string();

        let initial_balances: Vec<InitialBalance> = (0..num_owners)
            .into_iter()
            .map(|i| InitialBalance {
                address: format!("owner{}", i),
                amount: Uint128::new(5000),
            })
            .collect();
        let (init_result, mut deps) = init_helper(initial_balances);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );
        for i in 0..num_owners {
            let handle_msg = ExecuteMsg::SetViewingKey {
                key: vk.clone(),
                #[cfg(feature = "gas_evaporation")]
                gas_target: None,
                padding: None,
            };
            let info = mock_info(format!("owner{}", i).as_str(), &[]);

            let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

            let unwrapped_result: ExecuteAnswer =
                from_binary(&handle_result.unwrap().data.unwrap()).unwrap();
            assert_eq!(
                to_binary(&unwrapped_result).unwrap(),
                to_binary(&ExecuteAnswer::SetViewingKey {
                    status: ResponseStatus::Success
                })
                .unwrap(),
            );
        }

        for i in 0..num_owners {
            for j in 0..num_spenders {
                let handle_msg = ExecuteMsg::IncreaseAllowance {
                    spender: format!("spender{}", j),
                    amount: Uint128::new(50),
                    padding: None,
                    #[cfg(feature = "gas_evaporation")]
                    gas_target: None,
                    expiration: None,
                };
                let info = mock_info(format!("owner{}", i).as_str(), &[]);

                let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);
                assert!(
                    handle_result.is_ok(),
                    "handle() failed: {}",
                    handle_result.err().unwrap()
                );

                let handle_msg = ExecuteMsg::SetViewingKey {
                    key: vk.clone(),
                    #[cfg(feature = "gas_evaporation")]
                    gas_target: None,
                    padding: None,
                };
                let info = mock_info(format!("spender{}", j).as_str(), &[]);

                let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

                let unwrapped_result: ExecuteAnswer =
                    from_binary(&handle_result.unwrap().data.unwrap()).unwrap();
                assert_eq!(
                    to_binary(&unwrapped_result).unwrap(),
                    to_binary(&ExecuteAnswer::SetViewingKey {
                        status: ResponseStatus::Success
                    })
                    .unwrap(),
                );
            }
        }

        let query_msg = QueryMsg::AllowancesGiven {
            owner: "owner0".to_string(),
            key: vk.clone(),
            page: None,
            page_size: 5,
        };
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        match from_binary(&query_result.unwrap()).unwrap() {
            QueryAnswer::AllowancesGiven {
                owner,
                allowances,
                count,
            } => {
                assert_eq!(owner, "owner0".to_string());
                assert_eq!(allowances.len(), 5);
                assert_eq!(allowances[0].spender, "spender0");
                assert_eq!(allowances[0].allowance, Uint128::from(50_u128));
                assert_eq!(allowances[0].expiration, None);
                assert_eq!(count, num_spenders);
            }
            _ => panic!("Unexpected"),
        };

        let query_msg = QueryMsg::AllowancesGiven {
            owner: "owner1".to_string(),
            key: vk.clone(),
            page: Some(1),
            page_size: 5,
        };
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        match from_binary(&query_result.unwrap()).unwrap() {
            QueryAnswer::AllowancesGiven {
                owner,
                allowances,
                count,
            } => {
                assert_eq!(owner, "owner1".to_string());
                assert_eq!(allowances.len(), 5);
                assert_eq!(allowances[0].spender, "spender5");
                assert_eq!(allowances[0].allowance, Uint128::from(50_u128));
                assert_eq!(allowances[0].expiration, None);
                assert_eq!(count, num_spenders);
            }
            _ => panic!("Unexpected"),
        };

        let query_msg = QueryMsg::AllowancesGiven {
            owner: "owner1".to_string(),
            key: vk.clone(),
            page: Some(0),
            page_size: 23,
        };
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        match from_binary(&query_result.unwrap()).unwrap() {
            QueryAnswer::AllowancesGiven {
                owner,
                allowances,
                count,
            } => {
                assert_eq!(owner, "owner1".to_string());
                assert_eq!(allowances.len(), 20);
                assert_eq!(count, num_spenders);
            }
            _ => panic!("Unexpected"),
        };

        let query_msg = QueryMsg::AllowancesGiven {
            owner: "owner1".to_string(),
            key: vk.clone(),
            page: Some(2),
            page_size: 8,
        };
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        match from_binary(&query_result.unwrap()).unwrap() {
            QueryAnswer::AllowancesGiven {
                owner,
                allowances,
                count,
            } => {
                assert_eq!(owner, "owner1".to_string());
                assert_eq!(allowances.len(), 4);
                assert_eq!(count, num_spenders);
            }
            _ => panic!("Unexpected"),
        };

        let query_msg = QueryMsg::AllowancesGiven {
            owner: "owner2".to_string(),
            key: vk.clone(),
            page: Some(5),
            page_size: 5,
        };
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        match from_binary(&query_result.unwrap()).unwrap() {
            QueryAnswer::AllowancesGiven {
                owner,
                allowances,
                count,
            } => {
                assert_eq!(owner, "owner2".to_string());
                assert_eq!(allowances.len(), 0);
                assert_eq!(count, num_spenders);
            }
            _ => panic!("Unexpected"),
        };

        let query_msg = QueryMsg::AllowancesReceived {
            spender: "spender0".to_string(),
            key: vk.clone(),
            page: None,
            page_size: 10,
        };
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        match from_binary(&query_result.unwrap()).unwrap() {
            QueryAnswer::AllowancesReceived {
                spender,
                allowances,
                count,
            } => {
                assert_eq!(spender, "spender0".to_string());
                assert_eq!(allowances.len(), 3);
                assert_eq!(allowances[0].owner, "owner0");
                assert_eq!(allowances[0].allowance, Uint128::from(50_u128));
                assert_eq!(allowances[0].expiration, None);
                assert_eq!(count, num_owners);
            }
            _ => panic!("Unexpected"),
        };

        let query_msg = QueryMsg::AllowancesReceived {
            spender: "spender1".to_string(),
            key: vk.clone(),
            page: Some(1),
            page_size: 1,
        };
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        match from_binary(&query_result.unwrap()).unwrap() {
            QueryAnswer::AllowancesReceived {
                spender,
                allowances,
                count,
            } => {
                assert_eq!(spender, "spender1".to_string());
                assert_eq!(allowances.len(), 1);
                assert_eq!(allowances[0].owner, "owner1");
                assert_eq!(allowances[0].allowance, Uint128::from(50_u128));
                assert_eq!(allowances[0].expiration, None);
                assert_eq!(count, num_owners);
            }
            _ => panic!("Unexpected"),
        };
    }

    #[test]
    fn test_query_balance() {
        let (init_result, mut deps) = init_helper(vec![InitialBalance {
            address: "bob".to_string(),
            amount: Uint128::new(5000),
        }]);
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let handle_msg = ExecuteMsg::SetViewingKey {
            key: "key".to_string(),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let unwrapped_result: ExecuteAnswer =
            from_binary(&handle_result.unwrap().data.unwrap()).unwrap();
        assert_eq!(
            to_binary(&unwrapped_result).unwrap(),
            to_binary(&ExecuteAnswer::SetViewingKey {
                status: ResponseStatus::Success
            })
            .unwrap(),
        );

        let query_msg = QueryMsg::Balance {
            address: "bob".to_string(),
            key: "wrong_key".to_string(),
        };
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        let error = extract_error_msg(query_result);
        assert!(error.contains("Wrong viewing key"));

        let query_msg = QueryMsg::Balance {
            address: "bob".to_string(),
            key: "key".to_string(),
        };
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        let balance = match from_binary(&query_result.unwrap()).unwrap() {
            QueryAnswer::Balance { amount } => amount,
            _ => panic!("Unexpected"),
        };
        assert_eq!(balance, Uint128::new(5000));
    }

    #[test]
    fn test_query_transaction_history() {
        let (init_result, mut deps) = init_helper_with_config(
            vec![InitialBalance {
                address: "bob".to_string(),
                amount: Uint128::new(10000),
            }],
            true,
            true,
            true,
            true,
            1000,
            vec!["uscrt".to_string()],
        );
        assert!(
            init_result.is_ok(),
            "Init failed: {}",
            init_result.err().unwrap()
        );

        let handle_msg = ExecuteMsg::SetViewingKey {
            key: "key".to_string(),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(ensure_success(handle_result.unwrap()));

        let handle_msg = ExecuteMsg::Burn {
            amount: Uint128::new(1),
            memo: Some("my burn message".to_string()),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "Pause handle failed: {}",
            handle_result.err().unwrap()
        );

        let handle_msg = ExecuteMsg::Redeem {
            amount: Uint128::new(1000),
            denom: Option::from("uscrt".to_string()),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );

        let handle_msg = ExecuteMsg::Mint {
            recipient: "bob".to_string(),
            amount: Uint128::new(100),
            memo: Some("my mint message".to_string()),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("admin", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        assert!(ensure_success(handle_result.unwrap()));

        let handle_msg = ExecuteMsg::Deposit {
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info(
            "bob",
            &[Coin {
                denom: "uscrt".to_string(),
                amount: Uint128::new(1000),
            }],
        );

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);
        assert!(
            handle_result.is_ok(),
            "handle() failed: {}",
            handle_result.err().unwrap()
        );

        let handle_msg = ExecuteMsg::Transfer {
            recipient: "alice".to_string(),
            amount: Uint128::new(1000),
            memo: Some("my transfer message #1".to_string()),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let result = handle_result.unwrap();
        assert!(ensure_success(result));

        let handle_msg = ExecuteMsg::Transfer {
            recipient: "banana".to_string(),
            amount: Uint128::new(500),
            memo: Some("my transfer message #2".to_string()),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let result = handle_result.unwrap();
        assert!(ensure_success(result));

        let handle_msg = ExecuteMsg::Transfer {
            recipient: "mango".to_string(),
            amount: Uint128::new(2500),
            memo: Some("my transfer message #3".to_string()),
            #[cfg(feature = "gas_evaporation")]
            gas_target: None,
            padding: None,
        };
        let info = mock_info("bob", &[]);

        let handle_result = execute(deps.as_mut(), mock_env(), info, handle_msg);

        let result = handle_result.unwrap();
        assert!(ensure_success(result));

        let query_msg = QueryMsg::TransactionHistory {
            address: "bob".to_string(),
            key: "key".to_string(),
            page: None,
            page_size: 10,
        };
        let query_result = query(deps.as_ref(), mock_env(), query_msg);
        let transfers = match from_binary(&query_result.unwrap()).unwrap() {
            QueryAnswer::TransactionHistory { txs, .. } => txs,
            other => panic!("Unexpected: {:?}", other),
        };

        use crate::transaction_history::TxAction;
        let expected_transfers = [
            Tx {
                id: 8,
                action: TxAction::Transfer {
                    from: Addr::unchecked("bob".to_string()),
                    sender: Addr::unchecked("bob".to_string()),
                    recipient: Addr::unchecked("mango".to_string()),
                },
                coins: Coin {
                    denom: "SECSEC".to_string(),
                    amount: Uint128::new(2500),
                },
                memo: Some("my transfer message #3".to_string()),
                block_time: 1571797419,
                block_height: 12345,
            },
            Tx {
                id: 7,
                action: TxAction::Transfer {
                    from: Addr::unchecked("bob".to_string()),
                    sender: Addr::unchecked("bob".to_string()),
                    recipient: Addr::unchecked("banana".to_string()),
                },
                coins: Coin {
                    denom: "SECSEC".to_string(),
                    amount: Uint128::new(500),
                },
                memo: Some("my transfer message #2".to_string()),
                block_time: 1571797419,
                block_height: 12345,
            },
            Tx {
                id: 6,
                action: TxAction::Transfer {
                    from: Addr::unchecked("bob".to_string()),
                    sender: Addr::unchecked("bob".to_string()),
                    recipient: Addr::unchecked("alice".to_string()),
                },
                coins: Coin {
                    denom: "SECSEC".to_string(),
                    amount: Uint128::new(1000),
                },
                memo: Some("my transfer message #1".to_string()),
                block_time: 1571797419,
                block_height: 12345,
            },
            Tx {
                id: 5,
                action: TxAction::Deposit {},
                coins: Coin {
                    denom: "uscrt".to_string(),
                    amount: Uint128::new(1000),
                },
                memo: None,
                block_time: 1571797419,
                block_height: 12345,
            },
            Tx {
                id: 4,
                action: TxAction::Mint {
                    minter: Addr::unchecked("admin".to_string()),
                    recipient: Addr::unchecked("bob".to_string()),
                },
                coins: Coin {
                    denom: "SECSEC".to_string(),
                    amount: Uint128::new(100),
                },
                memo: Some("my mint message".to_string()),
                block_time: 1571797419,
                block_height: 12345,
            },
            Tx {
                id: 3,
                action: TxAction::Redeem {},
                coins: Coin {
                    denom: "SECSEC".to_string(),
                    amount: Uint128::new(1000),
                },
                memo: None,
                block_time: 1571797419,
                block_height: 12345,
            },
            Tx {
                id: 2,
                action: TxAction::Burn {
                    burner: Addr::unchecked("bob".to_string()),
                    owner: Addr::unchecked("bob".to_string()),
                },
                coins: Coin {
                    denom: "SECSEC".to_string(),
                    amount: Uint128::new(1),
                },
                memo: Some("my burn message".to_string()),
                block_time: 1571797419,
                block_height: 12345,
            },
            Tx {
                id: 1,
                action: TxAction::Mint {
                    minter: Addr::unchecked("admin".to_string()),
                    recipient: Addr::unchecked("bob".to_string()),
                },
                coins: Coin {
                    denom: "SECSEC".to_string(),
                    amount: Uint128::new(10000),
                },

                memo: Some("Initial Balance".to_string()),
                block_time: 1571797419,
                block_height: 12345,
            },
        ];

        assert_eq!(transfers, expected_transfers);
    }
}
