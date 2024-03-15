use std::collections::HashMap;

use cosmwasm_std::{Addr, Api, Binary, CanonicalAddr, Env, StdError, StdResult, Storage};
use primitive_types::{U256, U512};
use secret_toolkit::storage::Keyset;
use secret_toolkit_crypto::sha_256;
use serde::{Serialize, Deserialize};
use minicbor_ser as cbor;
use crate::{contract::{NOTIFICATION_BLOCK_SIZE, ZERO_ADDR}, crypto::{hkdf_sha_256, xor_bytes}, state::{get_seed, SNIP52_INTERNAL_SECRET}};
use hkdf::hmac::Mac;
use crate::crypto::{HmacSha256, hkdf_sha_512, cipher_data};

pub static CHANNELS: Keyset<String> = Keyset::new(b"channel-ids");

//  recvd = [
//      amount: biguint,   ; transfer amount in base denomination
//      sender: bstr,      ; byte sequence of sender's canonical address
//      balance: biguint   ; recipient's new balance after the transfer
//  ]

// id for the `recvd` channel
pub const RECEIVED_CHANNEL_ID: &str = "recvd";
// CDDL Schema for `recvd` channel data
pub const RECEIVED_CHANNEL_SCHEMA: &str = "recvd=[amount:biguint,sender:bstr,balance:biguint]";

#[derive(Serialize, Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(Eq, PartialEq))]
pub struct ReceivedNotification {
    // target recipient for the notification
    pub notification_for: Addr,
    // data
    pub amount: u128,
    pub sender: Option<Addr>,
    pub balance: u128,
}

impl ReceivedNotification {
    pub fn to_notification(self, api: &dyn Api, storage: &dyn Storage, block_height: u64, tx_hash: &String) -> StdResult<Snip52Notification> {
        let notification_for_raw = api.addr_canonicalize(self.notification_for.as_str())?;
        let channel = RECEIVED_CHANNEL_ID.to_string();
    
        // get notification id for receiver
        let received_id = notification_id(storage, &notification_for_raw, &channel, &tx_hash)?;
    
        // use CBOR to encode data
        let received_data;
        if let Some(sender) = self.sender {
            let sender_raw = api.addr_canonicalize(sender.as_str())?;
            received_data = cbor::to_vec(&(
                self.amount.to_be_bytes(),
                sender_raw.as_slice(),
                self.balance.to_be_bytes(),
            )).map_err(|e| 
                StdError::generic_err(format!("{:?}", e))
            )?;
        } else {
            received_data = cbor::to_vec(&(
                self.amount.to_be_bytes(),
                ZERO_ADDR,
                self.balance.to_be_bytes(),
            )).map_err(|e| 
                StdError::generic_err(format!("{:?}", e))
            )?;
        }
    
        // encrypt the receiver message
        let received_encrypted_data = encrypt_notification_data(
            storage,
            block_height,
            &tx_hash,
            &notification_for_raw,
            &channel,
            received_data
        )?;

        Ok(Snip52Notification {
            id: received_id,
            encrypted_data: received_encrypted_data,
        })
    }
}

//spent = [
//    amount: biguint,   ; transfer amount in base denomination
//    actions: uint      ; number of actions the execution performed
//    recipient: bstr,   ; byte sequence of first recipient's canonical address
//    balance: biguint   ; sender's new balance aactions: uint      ; number of actions the execution performedfter the transfer
//]

// id for the `spent` channel
pub const SPENT_CHANNEL_ID: &str = "spent";
// CDDL Schema for `spent` channel data
pub const SPENT_CHANNEL_SCHEMA: &str = "spent=[amount:biguint,actions:uint,recipient:bstr,balance:biguint]";

#[derive(Serialize, Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(Eq, PartialEq))]
pub struct SpentNotification {
    // target recipient for the notification
    pub notification_for: Addr,
    // data
    pub amount: u128,
    pub actions: u32,
    pub recipient: Option<Addr>,
    pub balance: u128,
}

impl SpentNotification {
    pub fn to_notification(self, api: &dyn Api, storage: &dyn Storage, block_height: u64, tx_hash: &String) -> StdResult<Snip52Notification> {
        let notification_for_raw = api.addr_canonicalize(self.notification_for.as_str())?;
        let channel = SPENT_CHANNEL_ID.to_string();
    
        // get notification id for spent
        let spent_id = notification_id(storage, &notification_for_raw, &channel, &tx_hash)?;

        // use CBOR to encode data
        let spent_data;
        if let Some(recipient) = self.recipient {
            let recipient_raw = api.addr_canonicalize(recipient.as_str())?;
            spent_data = cbor::to_vec(&(
                self.amount.to_be_bytes(),
                self.actions.to_be_bytes(),
                recipient_raw.as_slice(),
                self.balance.to_be_bytes(),
            )).map_err(|e| 
                StdError::generic_err(format!("{:?}", e))
            )?;
        } else {
            spent_data = cbor::to_vec(&(
                self.amount.to_be_bytes(),
                self.actions.to_be_bytes(),
                ZERO_ADDR,
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

        Ok(Snip52Notification {
            id: spent_id,
            encrypted_data: spent_encrypted_data,
        })
    }
}

//allowance = [
//    amount: biguint,   ; allowance amount in base denomination
//    allower: bstr,     ; byte sequence of allower's canonical address
//    expiration: uint,  ; epoch seconds of allowance expiration
//]

// id for the `allowance` channel
pub const ALLOWANCE_CHANNEL_ID: &str = "allowance";
// CDDL Schema for `allowance` channel data
pub const ALLOWANCE_CHANNEL_SCHEMA: &str = "allowance=[amount:biguint,allower:bstr,expiration:uint]";

#[derive(Serialize, Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(Eq, PartialEq))]
pub struct AllowanceNotification {
    // target recipient for the notification
    pub notification_for: Addr,
    // data
    pub amount: u128,
    pub allower: Addr,
    pub expiration: Option<u64>,
}

impl AllowanceNotification {
    pub fn to_notification(self, api: &dyn Api, storage: &dyn Storage, block_height: u64, tx_hash: &String) -> StdResult<Snip52Notification> {
        let notification_for_raw = api.addr_canonicalize(self.notification_for.as_str())?;
        let allower_raw = api.addr_canonicalize(self.allower.as_str())?;
        let channel = ALLOWANCE_CHANNEL_ID.to_string();

        // get notification id for receiver of allowance
        let updated_allowance_id = notification_id(storage, &notification_for_raw, &channel, &tx_hash)?;

        // use CBOR to encode data
        let updated_allowance_data = cbor::to_vec(&(
            self.amount.to_be_bytes(),
            allower_raw.as_slice(),
            self.expiration.unwrap_or(0u64), // expiration == 0 means no expiration
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

        Ok(Snip52Notification {
            id: updated_allowance_id,
            encrypted_data: updated_allowance_encrypted_data,
        })
    }
}

#[derive(Serialize, Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(Eq, PartialEq))]
pub struct Snip52Notification {
    pub id: Binary,
    pub encrypted_data: Binary,
}

impl Snip52Notification {
    pub fn id_plaintext(&self) -> String {
        format!("snip52:{}", self.id.to_base64())
    }

    pub fn data_plaintext(&self) -> String {
        self.encrypted_data.to_base64()
    }
}

// id for the `multirecvd` channel
pub const MULTI_RECEIVED_CHANNEL_ID: &str = "multirecvd";
pub const MULTI_RECEIVED_CHANNEL_BLOOM_K: u32 = 15;
pub const MULTI_RECEIVED_CHANNEL_BLOOM_N: u32 = 16;
pub const MULTI_RECEIVED_CHANNEL_PACKET_SIZE: u32 = 40;

// id for the `multispent` channel
pub const MULTI_SPENT_CHANNEL_ID: &str = "multispent";
pub const MULTI_SPENT_CHANNEL_BLOOM_K: u32 = 5;
pub const MULTI_SPENT_CHANNEL_BLOOM_N: u32 = 4;
pub const MULTI_SPENT_CHANNEL_PACKET_SIZE: u32 = 40;

pub fn multi_received_bloom_value(
    storage: &dyn Storage,
    api: &dyn Api,
    notifications: Vec<ReceivedNotification>,
    tx_hash: &String,
    env_random: Binary,
) -> StdResult<Vec<u8>> {
    let mut received_bloom_filter: U512 = U512::from(0);
    let mut received_packets: Vec<(Addr, Vec<u8>)> = vec![];

    // keep track of how often addresses might show up in packet data.
    // we need to remove any address that might show up more than once.
    let mut recipient_counts: HashMap<Addr,u16> = HashMap::new();

    for notification in &notifications {
        recipient_counts.insert(
            notification.notification_for.clone(),
            recipient_counts.get(&notification.notification_for).unwrap_or(&0u16) + 1
        );

        // contribute to received bloom filter
        let recipient_addr_raw = api.addr_canonicalize(notification.notification_for.as_str())?;
        let id = notification_id(
            storage, 
            &recipient_addr_raw, 
            &MULTI_RECEIVED_CHANNEL_ID.to_string(), 
            &tx_hash
        )?;
        let mut hash_bytes = U256::from_big_endian(&sha_256(id.0.as_slice()));
        for _ in 0..MULTI_RECEIVED_CHANNEL_BLOOM_K {
            let bit_index = (hash_bytes & U256::from(0x01ff)).as_usize();
            received_bloom_filter = received_bloom_filter | (U512::from(1) << bit_index);
            hash_bytes = hash_bytes >> 9;
        }

        // now that we've update the bloom filter we can short circuit this if recipient count > 1, 
        // since we will throw this packet out anyway
        if *recipient_counts.get(&notification.notification_for).unwrap() > 1 { continue; }

        // make the received packet
        let mut received_packet_plaintext: Vec<u8> = vec![];
        // amount bytes (u128 == 16 bytes)
        received_packet_plaintext.extend_from_slice(&notification.amount.to_be_bytes());
        // balance bytes (u128 == 16 bytes)
        received_packet_plaintext.extend_from_slice(&notification.balance.to_be_bytes());
        // sender account last 8 bytes
        let sender_bytes: &[u8];
        let sender_raw;
        if let Some(sender) = &notification.sender {
            sender_raw = api.addr_canonicalize(sender.as_str())?;
            sender_bytes = &sender_raw.as_slice()[sender_raw.0.len()-8..];
        } else {
            sender_bytes = &ZERO_ADDR[ZERO_ADDR.len()-8..];
        }
        received_packet_plaintext.extend_from_slice(sender_bytes);

        let received_packet_size = received_packet_plaintext.len();
        let received_packet_id = &id.0.as_slice()[0..8];
        let received_packet_ikm = &id.0.as_slice()[8..32];
        let received_packet_key = hkdf_sha_512(
            &Some(vec![0u8; 64]), 
            received_packet_ikm, 
            "".as_bytes(), 
            received_packet_size
        )?;
        let received_packet_ciphertext = xor_bytes(received_packet_plaintext.as_slice(), received_packet_key.as_slice());
        let received_packet_bytes: Vec<u8> = [received_packet_id.to_vec(), received_packet_ciphertext].concat();

        received_packets.push((notification.notification_for.clone(), received_packet_bytes));
    }

    // filter out any notifications for recipients showing up more than once
    let mut received_packets: Vec<Vec<u8>> = received_packets
        .into_iter()
        .filter(|(addr, _)| *recipient_counts.get(addr).unwrap_or(&0u16) <= 1)
        .map(|(_, packet)| packet)
        .collect();
    if received_packets.len() > MULTI_RECEIVED_CHANNEL_BLOOM_N as usize { // still too many packets
        received_packets = received_packets[0..MULTI_RECEIVED_CHANNEL_BLOOM_N as usize].to_vec();
    }
    // now add decoy packets
    let num_decoys = MULTI_RECEIVED_CHANNEL_BLOOM_N.saturating_sub(received_packets.len() as u32) as usize;
    if num_decoys > 0 {
        let decoy_packet_ids = hkdf_sha_512(
            &Some(vec![0u8; 64]),
            &env_random,
            format!("{}:decoys", MULTI_RECEIVED_CHANNEL_ID).as_bytes(), 
            num_decoys * 8,
        )?;
        for i in 0..num_decoys {
            let decoy_packet = [
                decoy_packet_ids[i*8..(i+1)*8].to_vec(),
                vec![0u8; MULTI_RECEIVED_CHANNEL_PACKET_SIZE as usize]
            ].concat();
            received_packets.push(decoy_packet);
        }
    }
    
    let mut received_bloom_filter_bytes: Vec<u8> = vec![];
    for biguint in received_bloom_filter.0 {
        received_bloom_filter_bytes.extend_from_slice(&biguint.to_be_bytes());
    }
    for packet in received_packets {
        received_bloom_filter_bytes.extend(packet.iter());
    }

    Ok(received_bloom_filter_bytes)
}

pub fn multi_spent_bloom_value(
    storage: &dyn Storage,
    api: &dyn Api,
    notifications: Vec<SpentNotification>,
    tx_hash: &String,
    env_random: Binary,
) -> StdResult<Vec<u8>> {
    let mut spent_bloom_filter: U512 = U512::from(0);
    let mut spent_packets: Vec<(Addr, Vec<u8>)> = vec![];

    // keep track of how often addresses might show up in packet data.
    // we need to remove any address that might show up more than once.
    let mut spent_counts: HashMap<Addr,u16> = HashMap::new();

    for notification in &notifications {
        spent_counts.insert(
            notification.notification_for.clone(),
            spent_counts.get(&notification.notification_for).unwrap_or(&0u16) + 1
        );

        let spender_addr_raw = api.addr_canonicalize(notification.notification_for.as_str())?;
        let id = notification_id(
            storage, 
            &spender_addr_raw, 
            &MULTI_SPENT_CHANNEL_ID.to_string(), 
            &tx_hash
        )?;
        let mut hash_bytes = U256::from_big_endian(&sha_256(id.0.as_slice()));
        for _ in 0..MULTI_SPENT_CHANNEL_BLOOM_K {
            let bit_index = (hash_bytes & U256::from(0x01ff)).as_usize();
            spent_bloom_filter = spent_bloom_filter | (U512::from(1) << bit_index);
            hash_bytes = hash_bytes >> 9;
        }

        // now that we've update the bloom filter we can short circuit this if recipient count > 1, 
        // since we will throw this packet out anyway
        if *spent_counts.get(&notification.notification_for).unwrap() > 1 { continue; }

        // make the spent packet
        let mut spent_packet_plaintext: Vec<u8> = vec![];
        // amount bytes (u128 == 16 bytes)
        spent_packet_plaintext.extend_from_slice(&notification.amount.to_be_bytes());
        // balance bytes (u128 == 16 bytes)
        spent_packet_plaintext.extend_from_slice(&notification.balance.to_be_bytes());
        // recipient account last 8 bytes 
        let recipient_bytes: &[u8];
        let recipient_raw;
        if let Some(recipient) = &notification.recipient {
            recipient_raw = api.addr_canonicalize(recipient.as_str())?;
            recipient_bytes = &recipient_raw.as_slice()[recipient_raw.0.len()-8..];
        } else {
            recipient_bytes = &ZERO_ADDR[ZERO_ADDR.len()-8..];
        }
        spent_packet_plaintext.extend_from_slice(recipient_bytes);

        let spent_packet_size = spent_packet_plaintext.len();
        let spent_packet_id = &id.0.as_slice()[0..8];
        let spent_packet_ikm = &id.0.as_slice()[8..32];
        let spent_packet_key = hkdf_sha_512(
            &Some(vec![0u8; 64]),
            spent_packet_ikm, 
            "".as_bytes(), 
            spent_packet_size
        )?;
        let spent_packet_ciphertext = xor_bytes(spent_packet_plaintext.as_slice(), spent_packet_key.as_slice());
        let spent_packet_bytes: Vec<u8> = [spent_packet_id.to_vec(), spent_packet_ciphertext].concat(); 

        spent_packets.push((notification.notification_for.clone(), spent_packet_bytes));
    }

    // filter out any notifications for recipients showing up more than once
    let mut spent_packets: Vec<Vec<u8>> = spent_packets
        .into_iter()
        .filter(|(addr, _)| *spent_counts.get(addr).unwrap_or(&0u16) <= 1)
        .map(|(_, packet)| packet)
        .collect();
    if spent_packets.len() > MULTI_SPENT_CHANNEL_BLOOM_N as usize { // still too many packets
        spent_packets = spent_packets[0..MULTI_SPENT_CHANNEL_BLOOM_N as usize].to_vec();
    }
    // now add decoy packets
    let num_decoys = MULTI_SPENT_CHANNEL_BLOOM_N.saturating_sub(spent_packets.len() as u32) as usize;
    if num_decoys > 0 {
        let decoy_packet_ids = hkdf_sha_512(
            &Some(vec![0u8; 64]),
            &env_random,
            format!("{}:decoys", MULTI_SPENT_CHANNEL_ID).as_bytes(), 
            num_decoys * 8,
        )?;
        for i in 0..num_decoys {
            let decoy_packet = [
                decoy_packet_ids[i*8..(i+1)*8].to_vec(),
                vec![0u8; MULTI_SPENT_CHANNEL_PACKET_SIZE as usize]
            ].concat();
            spent_packets.push(decoy_packet);
        }
    }
    
    let mut spent_bloom_filter_bytes: Vec<u8> = vec![];
    for biguint in spent_bloom_filter.0 {
        spent_bloom_filter_bytes.extend_from_slice(&biguint.to_be_bytes());
    }
    for packet in spent_packets {
        spent_bloom_filter_bytes.extend(packet.iter());
    }

    Ok(spent_bloom_filter_bytes)
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
) -> StdResult<Snip52Notification> {
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
    Ok(Snip52Notification{
        id,
        encrypted_data,
    })
}

pub fn update_batch_notifications_to_final_balance(
    notifications: Vec<(ReceivedNotification, SpentNotification)>,
) -> Vec<(ReceivedNotification, SpentNotification)> {
    let mut final_balances: HashMap<Addr, u128> = HashMap::new();
    notifications.iter().for_each(|notification| {
        final_balances.insert(notification.0.notification_for.clone(), notification.0.balance);
        final_balances.insert(notification.1.notification_for.clone(), notification.1.balance);
    });
    // update with final balance for all notifications
    let notifications: Vec<(ReceivedNotification, SpentNotification)> = notifications
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