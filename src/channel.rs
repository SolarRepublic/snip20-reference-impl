use std::collections::HashMap;

use cosmwasm_std::{Addr, Api, Binary, CanonicalAddr, Env, StdError, StdResult, Storage};
use secret_toolkit::storage::{Keyset, Keymap};
use secret_toolkit_crypto::sha_256;
use serde::{Serialize, Deserialize};
use minicbor_ser as cbor;
use crate::{contract::{BURN_ADDR, NOTIFICATION_BLOCK_SIZE}, state::get_seed};
use hkdf::hmac::Mac;
use crate::crypto::{HmacSha256, cipher_data};

pub static CHANNELS: Keyset<String> = Keyset::new(b"channel-ids");
pub static CHANNEL_SCHEMATA: Keymap<String,String> = Keymap::new(b"channel-schemata");

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Channel {
    pub id: String,
    pub schema: Option<String>,
}

impl Channel {
    pub fn store(self, storage: &mut dyn Storage) -> StdResult<()> {
        CHANNELS.insert(storage, &self.id)?;
        if let Some(schema) = self.schema {
            CHANNEL_SCHEMATA.insert(storage, &self.id, &schema)?;
        } else if CHANNEL_SCHEMATA.get(storage, &self.id).is_some() { 
            // double check it does not already have a schema stored, and if 
            //   it does remove it.
            CHANNEL_SCHEMATA.remove(storage, &self.id)?;
        }
        Ok(())
    }
}

//  received_tokens = [
//      amount: biguint,   ; transfer amount in base denomination
//      sender: bstr,      ; byte sequence of sender's canonical address
//      balance: biguint   ; recipient's new balance after the transfer
//  ]

// id for the `received_tokens` channel
pub const RECEIVED_TOKENS_CHANNEL_ID: &str = "received_tokens";
// CDDL Schema for `received_tokens` channel data
pub const RECEIVED_TOKENS_CHANNEL_SCHEMA: &str = "received_tokens=[amount:biguint,sender:bstr,balance:biguint]";

#[derive(Serialize, Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(Eq, PartialEq))]
pub struct ReceivedTokensNotification {
    // target recipient for the notification
    pub notification_for: Addr,
    // data
    pub amount: u128,
    pub sender: Addr,
    pub balance: u128,
}

impl ReceivedTokensNotification {
    pub fn to_notification(self, api: &dyn Api, storage: &dyn Storage, block_height: u64, tx_hash: &String) -> StdResult<Notification> {
        let notification_for_raw = api.addr_canonicalize(self.notification_for.as_str())?;
        let sender_raw = api.addr_canonicalize(self.sender.as_str())?;
        let channel = RECEIVED_TOKENS_CHANNEL_ID.to_string();
    
        // get notification id for receiver
        let received_id = notification_id(storage, &notification_for_raw, &channel, &tx_hash)?;
    
        // use CBOR to encode data
        let received_data = cbor::to_vec(&(
            self.amount.to_be_bytes(),
            sender_raw.as_slice(),
            self.balance.to_be_bytes(),
        )).map_err(|e| 
            StdError::generic_err(format!("{:?}", e))
        )?;
    
        // encrypt the receiver message
        let received_encrypted_data = encrypt_notification_data(
            storage,
            block_height,
            &tx_hash,
            &notification_for_raw,
            &channel,
            received_data
        )?;

        Ok(Notification {
            id: received_id,
            encrypted_data: received_encrypted_data,
        })
    }
}

//spent_tokens = [
//    amount: biguint,   ; transfer amount in base denomination
//    recipient: bstr,   ; byte sequence of recipient's canonical address
//    balance: biguint   ; sender's new balance after the transfer
//]

// id for the `spent_tokens` channel
pub const SPENT_TOKENS_CHANNEL_ID: &str = "spent_tokens";
// CDDL Schema for `spent_tokens` channel data
pub const SPENT_TOKENS_CHANNEL_SCHEMA: &str = "spent_tokens=[amount:biguint,recipient:bstr,balance:biguint]";

#[derive(Serialize, Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(Eq, PartialEq))]
pub struct SpentTokensNotification {
    // target recipient for the notification
    pub notification_for: Addr,
    // data
    pub amount: u128,
    pub recipient: Option<Addr>,
    pub balance: u128,
}

impl SpentTokensNotification {
    pub fn to_notification(self, api: &dyn Api, storage: &dyn Storage, block_height: u64, tx_hash: &String) -> StdResult<Notification> {
        let notification_for_raw = api.addr_canonicalize(self.notification_for.as_str())?;
        let channel = SPENT_TOKENS_CHANNEL_ID.to_string();
    
        // get notification id for spent
        let spent_id = notification_id(storage, &notification_for_raw, &channel, &tx_hash)?;

        // use CBOR to encode data
        let spent_data;
        if let Some(recipient) = self.recipient {
            let recipient_raw = api.addr_canonicalize(recipient.as_str())?;
            spent_data = cbor::to_vec(&(
                self.amount.to_be_bytes(),
                recipient_raw.as_slice(),
                self.balance.to_be_bytes(),
            )).map_err(|e| 
                StdError::generic_err(format!("{:?}", e))
            )?;
        } else {
            spent_data = cbor::to_vec(&(
                self.amount.to_be_bytes(),
                BURN_ADDR,
                self.balance.to_be_bytes(),
            )).map_err(|e| 
                StdError::generic_err(format!("{:?}", e))
            )?;
        }

        // encrypt the receiver message
        let spent_encrypted_data = encrypt_notification_data(
            storage,
            block_height,
            &tx_hash,
            &notification_for_raw,
            &channel,
            spent_data
        )?;

        Ok(Notification {
            id: spent_id,
            encrypted_data: spent_encrypted_data,
        })
    }
}

//updated_allowance = [
//    amount: biguint,   ; allowance amount in base denomination
//    allower: bstr,     ; byte sequence of allower's canonical address
//    expiration: uint,  ; epoch seconds of allowance expiration
//]

// id for the `updated_allowance` channel
pub const UPDATED_ALLOWANCE_CHANNEL_ID: &str = "updated_allowance";
// CDDL Schema for `updated_allowance` channel data
pub const UPDATED_ALLOWANCE_CHANNEL_SCHEMA: &str = "updated_allowance=[amount:biguint,allower:bstr,expiration:uint]";

#[derive(Serialize, Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(Eq, PartialEq))]
pub struct UpdatedAllowanceNotification {
    // target recipient for the notification
    pub notification_for: Addr,
    // data
    pub amount: u128,
    pub allower: Addr,
    pub expiration: Option<u64>,
}

impl UpdatedAllowanceNotification {
    pub fn to_notification(self, api: &dyn Api, storage: &dyn Storage, block_height: u64, tx_hash: &String) -> StdResult<Notification> {
        let notification_for_raw = api.addr_canonicalize(self.notification_for.as_str())?;
        let allower_raw = api.addr_canonicalize(self.allower.as_str())?;
        let channel = UPDATED_ALLOWANCE_CHANNEL_ID.to_string();

        // get notification id for receiver of allowance
        let updated_allowance_id = notification_id(storage, &notification_for_raw, &channel, &tx_hash)?;

        // use CBOR to encode data
        let updated_allowance_data = cbor::to_vec(&(
            self.amount.to_be_bytes(),
            allower_raw.as_slice(),
            self.expiration.unwrap_or_default(),
        )).map_err(|e| 
            StdError::generic_err(format!("{:?}", e))
        )?;

        // encrypt the updated allowance message
        let updated_allowance_encrypted_data = encrypt_notification_data(
            storage,
            block_height,
            &tx_hash,
            &notification_for_raw,
            &channel,
            updated_allowance_data
        )?;

        Ok(Notification {
            id: updated_allowance_id,
            encrypted_data: updated_allowance_encrypted_data,
        })
    }
}
 

#[derive(Serialize, Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(Eq, PartialEq))]
pub struct Notification {
    pub id: Binary,
    pub encrypted_data: Binary,
}

impl Notification {
    pub fn id_plaintext(&self) -> String {
        format!("snip52:{}", self.id.to_base64())
    }

    pub fn data_plaintext(&self) -> String {
        self.encrypted_data.to_base64()
    }
}

/// 
/// fn notification_id
/// 
///   Returns a notification id for the given address and channel id.
/// 
pub fn notification_id(
    storage: &dyn Storage,
    addr: &CanonicalAddr,
    channel: &String,
    tx_hash: &String,
) -> StdResult<Binary> {
    // compute notification ID for this event
    let seed = get_seed(storage, addr)?;
    let material = [
        channel.as_bytes(),
        ":".as_bytes(),
        tx_hash.as_bytes()
    ].concat();

    let mut mac: HmacSha256 = HmacSha256::new_from_slice(seed.0.as_slice()).unwrap();
    mac.update(material.as_slice());
    let result = mac.finalize();
    let code_bytes = result.into_bytes();
    Ok(Binary::from(code_bytes.as_slice()))
}

/// 
/// fn encrypt_notification_data
/// 
///   Returns encrypted bytes given plaintext bytes, address, and channel id.
/// 
pub fn encrypt_notification_data(
    storage: &dyn Storage,
    block_height: u64,
    tx_hash: &String,
    recipient: &CanonicalAddr,
    channel: &String,
    plaintext: Vec<u8>,
) -> StdResult<Binary> {
    let mut padded_plaintext = plaintext.clone();
    zero_pad(&mut padded_plaintext, NOTIFICATION_BLOCK_SIZE);

    let seed = get_seed(storage, recipient)?;

    let channel_id_bytes = sha_256(channel.as_bytes())[..12].to_vec();
    let salt_bytes = tx_hash.as_bytes()[..12].to_vec();
    let nonce: Vec<u8> = channel_id_bytes.iter().zip(salt_bytes.iter()).map(|(&b1, &b2)| b1 ^ b2 ).collect();
    let aad = format!("{}:{}", block_height, tx_hash);

    // encrypt notification data for this event
    let tag_ciphertext = cipher_data(
        seed.0.as_slice(),
        nonce.as_slice(),
        padded_plaintext.as_slice(),
        aad.as_bytes()
    )?;

    Ok(Binary::from(tag_ciphertext.clone()))
}

/// Take a Vec<u8> and pad it up to a multiple of `block_size`, using 0x00 at the end.
fn zero_pad(message: &mut Vec<u8>, block_size: usize) -> &mut Vec<u8> {
    let len = message.len();
    let surplus = len % block_size;
    if surplus == 0 {
        return message;
    }

    let missing = block_size - surplus;
    message.reserve(missing);
    message.extend(std::iter::repeat(0x00).take(missing));
    message
}

fn decoy_notification(
    api: &dyn Api,
    env: &Env,
    store: &mut dyn Storage,
    channel: String,
) -> StdResult<Notification> {
    let tx_hash = env.transaction.clone().ok_or(StdError::generic_err("no tx hash found"))?.hash;
    let contract_raw = api.addr_canonicalize(env.contract.address.as_str())?;
    let id = notification_id(store, &contract_raw, &channel, &tx_hash)?;
    let data = vec![];
    let encrypted_data = encrypt_notification_data(
        store,
        env.block.height,
        &tx_hash,
        &contract_raw,
        &channel,
        data
    )?;
    Ok(Notification{
        id,
        encrypted_data,
    })
}

pub fn update_batch_notifications_to_final_balance(
    notifications: Vec<(ReceivedTokensNotification, SpentTokensNotification)>,
) -> Vec<(ReceivedTokensNotification, SpentTokensNotification)> {
    let mut final_balances: HashMap<Addr, u128> = HashMap::new();
    notifications.iter().for_each(|notification| {
        final_balances.insert(notification.0.notification_for.clone(), notification.0.balance);
        final_balances.insert(notification.1.notification_for.clone(), notification.1.balance);
    });
    // update with final balance for all notifications
    let notifications: Vec<(ReceivedTokensNotification,SpentTokensNotification)> = notifications
        .into_iter()
        .map(|notification| {
            let mut new_notification = notification.clone();
            if let Some(final_balance) = final_balances.get(&notification.0.notification_for) {
                if notification.0.balance != *final_balance {
                    new_notification.0.balance = *final_balance;
                }
            }
            if let Some(final_balance) = final_balances.get(&notification.1.notification_for) {
                if notification.1.balance != *final_balance {
                    new_notification.1.balance = *final_balance;
                }
            }
            new_notification
        }
    ).collect();
    
    notifications
}