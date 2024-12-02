use std::collections::HashMap;

use cosmwasm_std::{Addr, Api, Binary, CanonicalAddr, StdError, StdResult};
use primitive_types::{U256, U512};
use secret_toolkit::notification::{get_seed, notification_id, xor_bytes, Notification, NotificationData, cbor_to_std_error};
use minicbor::{data as cbor_data, encode as cbor_encode, Encoder};
use secret_toolkit_crypto::{hkdf_sha_512, sha_256};
use serde::{Deserialize, Serialize};

const ZERO_ADDR: [u8; 20] = [0u8; 20];

const CBL_ARRAY: usize = 1 + 1;
const CBL_U32: usize = 1 + 4;
const CBL_BIGNUM_U64: usize = 1 + 8;
const CBL_TIMESTAMP: usize = 1 + 8;
const CBL_ADDRESS: usize = 1 + 20;

pub trait EncoderExt {
    fn bignum_u64(&mut self, value: u128) -> StdResult<&mut Self>;
    fn address(&mut self, value: CanonicalAddr) -> StdResult<&mut Self>;
    fn slice(&mut self, value: &[u8]) -> StdResult<&mut Self>;
    fn timestamp(&mut self, value: u64) -> StdResult<&mut Self>;
}

impl<T: cbor_encode::Write> EncoderExt for Encoder<T> {
    fn bignum_u64(&mut self, value: u128) -> StdResult<&mut Self> {
        self
            .tag(cbor_data::Tag::from(cbor_data::IanaTag::PosBignum))
                .map_err(cbor_to_std_error)?
            .slice(&value.to_be_bytes()[8..])
    }

    fn address(&mut self, value: CanonicalAddr) -> StdResult<&mut Self> {
        self.slice(&value.as_slice())
    }

    fn slice(&mut self, value: &[u8]) -> StdResult<&mut Self> {
        self
            .bytes(&value)
                .map_err(cbor_to_std_error)?;
        
        Ok(self)
    }

    fn timestamp(&mut self, value: u64) -> StdResult<&mut Self> {
        self
            .tag(cbor_data::Tag::from(cbor_data::IanaTag::Timestamp))
                .map_err(cbor_to_std_error)?
            .u64(value)
                .map_err(cbor_to_std_error)?;

        Ok(self)
    }
}

#[derive(Serialize, Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(Eq, PartialEq))]
pub struct ReceivedNotificationData {
    pub amount: u128,
    pub sender: Option<Addr>,
}

impl NotificationData for ReceivedNotificationData {
	const CHANNEL_ID: &'static str = "recvd";
	const CDDL_SCHEMA: &'static str = "recvd=[amount:biguint,sender:bstr]";
    const ELEMENTS: u64 = 2;
    const PAYLOAD_SIZE: usize = CBL_ARRAY + CBL_BIGNUM_U64 + CBL_ADDRESS;

    fn encode_cbor(&self, api: &dyn Api, encoder: &mut Encoder<&mut [u8]>) -> StdResult<()> {
        encoder.bignum_u64(self.amount)?;

        if let Some(sender) = &self.sender {
            let sender_raw = api.addr_canonicalize(sender.as_str())?;
            encoder.address(sender_raw)?;
        } else {
            encoder.slice(&ZERO_ADDR)?;
        }

        Ok(())
    }
}

// spent = [
//     amount: biguint,   ; transfer amount in base denomination
//     actions: uint      ; number of actions the execution performed
//     recipient: bstr,   ; byte sequence of first recipient's canonical address
//     balance: biguint   ; sender's new balance aactions
// ]

#[derive(Serialize, Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(Eq, PartialEq))]
pub struct SpentNotificationData {
    pub amount: u128,
    pub actions: u32,
    pub recipient: Option<Addr>,
    pub balance: u128,
}


impl NotificationData for SpentNotificationData {
    const CHANNEL_ID: &'static str = "spent";
	const CDDL_SCHEMA: &'static str = "spent=[amount:biguint,actions:uint,recipient:bstr,balance:biguint]";
    const ELEMENTS: u64 = 4;
    const PAYLOAD_SIZE: usize = CBL_ARRAY + CBL_BIGNUM_U64 + CBL_U32 + CBL_ADDRESS + CBL_BIGNUM_U64;

    fn encode_cbor(&self, api: &dyn Api, encoder: &mut Encoder<&mut [u8]>) -> StdResult<()> {
        let mut spent_data = encoder
            .bignum_u64(self.amount)?
            .u32(self.actions).map_err(cbor_to_std_error)?;

        if let Some(recipient) = &self.recipient {
            let recipient_raw = api.addr_canonicalize(recipient.as_str())?;
            spent_data = spent_data.address(recipient_raw)?;
        } else {
            spent_data = spent_data.slice(&ZERO_ADDR)?
        }

        spent_data.bignum_u64(self.balance)?;
        
        Ok(())
    }
}

//allowance = [
//    amount: biguint,   ; allowance amount in base denomination
//    allower: bstr,     ; byte sequence of allower's canonical address
//    expiration: uint,  ; epoch seconds of allowance expiration
//]

#[derive(Serialize, Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(Eq, PartialEq))]
pub struct AllowanceNotificationData {
    pub amount: u128,
    pub allower: Addr,
    pub expiration: Option<u64>,
}

impl NotificationData for AllowanceNotificationData {
    const CHANNEL_ID: &'static str = "allowance";
    const CDDL_SCHEMA: &'static str = "allowance=[amount:biguint,allower:bstr,expiration:uint]";
    const ELEMENTS: u64 = 3;
    const PAYLOAD_SIZE: usize = CBL_ARRAY + CBL_BIGNUM_U64 + CBL_ADDRESS + CBL_TIMESTAMP;

    fn encode_cbor(&self, api: &dyn Api, encoder: &mut Encoder<&mut [u8]>) -> StdResult<()> {
        let allower_raw = api.addr_canonicalize(self.allower.as_str())?;

        encoder
            .bignum_u64(self.amount)?
            .slice(allower_raw.as_slice())?
            .timestamp(self.expiration.unwrap_or(0u64))?;

        Ok(())
    }
}

// multi recipient push notifications

// id for the `multirecvd` channel
pub const MULTI_RECEIVED_CHANNEL_ID: &str = "multirecvd";
pub const MULTI_RECEIVED_CHANNEL_BLOOM_K: u32 = 15;
pub const MULTI_RECEIVED_CHANNEL_BLOOM_N: u32 = 16;
pub const MULTI_RECEIVED_CHANNEL_PACKET_SIZE: usize = 16;

// id for the `multispent` channel
pub const MULTI_SPENT_CHANNEL_ID: &str = "multispent";
pub const MULTI_SPENT_CHANNEL_BLOOM_K: u32 = 5;
pub const MULTI_SPENT_CHANNEL_BLOOM_N: usize = 4;
pub const MULTI_SPENT_CHANNEL_PACKET_SIZE: usize = 24;

pub fn multi_recvd_data(
    api: &dyn Api,
    notifications: Vec<Notification<ReceivedNotificationData>>,
    tx_hash: &String,
    env_random: Binary,
    secret: &[u8],
) -> StdResult<Vec<u8>> {
    let mut received_bloom_filter: U512 = U512::from(0);
    let mut received_packets: Vec<(Addr, Vec<u8>)> = vec![];

    // keep track of how often addresses might show up in packet data.
    // we need to remove any address that might show up more than once.
    let mut recipient_counts: HashMap<Addr, u16> = HashMap::new();

    for notification in &notifications {
        recipient_counts.insert(
            notification.notification_for.clone(),
            recipient_counts
                .get(&notification.notification_for)
                .unwrap_or(&0u16)
                + 1,
        );

        // we can short circuit this if recipient count > 1, since we will throw out this packet
        // anyway, and address has already been added to bloom filter
        if *recipient_counts
            .get(&notification.notification_for)
            .unwrap()
            > 1
        {
            continue;
        }

        // contribute to received bloom filter
        let recipient_addr_raw = api.addr_canonicalize(notification.notification_for.as_str())?;
        let seed = get_seed(&recipient_addr_raw, secret)?;
        let id = notification_id(&seed, &MULTI_RECEIVED_CHANNEL_ID.to_string(), &tx_hash)?;
        let hash_bytes = U256::from_big_endian(&sha_256(id.0.as_slice()));
        let bloom_mask: U256 = U256::from(0x1ff);
        for i in 0..MULTI_RECEIVED_CHANNEL_BLOOM_K {
            let bit_index = ((hash_bytes >> (256 - 9 - (i * 9))) & bloom_mask).as_usize();
            received_bloom_filter |= U512::from(1) << bit_index;
        }

        // make the received packet
        let mut packet_plaintext = [0u8; MULTI_RECEIVED_CHANNEL_PACKET_SIZE];

        // amount bytes (u64 == 8 bytes)
        packet_plaintext[..8].copy_from_slice(
            &notification.data.amount
                .clamp(0, u64::MAX as u128)
                .to_be_bytes()[8..]
        );

        // sender account last 8 bytes
        let sender_bytes: &[u8];
        let sender_raw;
        if let Some(sender) = &notification.data.sender {
            sender_raw = api.addr_canonicalize(sender.as_str())?;
            sender_bytes = &sender_raw.as_slice()[sender_raw.0.len() - 8..];
        } else {
            sender_bytes = &ZERO_ADDR[ZERO_ADDR.len() - 8..];
        }

        // 16 bytes total
        packet_plaintext[8..].copy_from_slice(sender_bytes);

        // use top 64 bits of notification ID for packet ID
        let packet_id = &id.0.as_slice()[0..8];

        // take the bottom bits from the notification ID for key material
        let packet_ikm = &id.0.as_slice()[8..32];

        // create ciphertext by XOR'ing the plaintext with the notification ID
        let packet_ciphertext = xor_bytes(
            &packet_plaintext[..],
            &packet_ikm[0..MULTI_RECEIVED_CHANNEL_PACKET_SIZE]
        );

        // construct the packet bytes
        let packet_bytes: Vec<u8> = [
            packet_id.to_vec(),
            packet_ciphertext,
        ].concat();

        // add to packets data
        received_packets.push((notification.notification_for.clone(), packet_bytes));
    }

    // filter out any notifications for recipients showing up more than once
    let mut received_packets: Vec<Vec<u8>> = received_packets
        .into_iter()
        .filter(|(addr, _)| *recipient_counts.get(addr).unwrap_or(&0u16) <= 1)
        .map(|(_, packet)| packet)
        .collect();

    // still too many packets; trim down to size
    if received_packets.len() > MULTI_RECEIVED_CHANNEL_BLOOM_N as usize {
        received_packets = received_packets[0..MULTI_RECEIVED_CHANNEL_BLOOM_N as usize].to_vec();
    }

    // now add extra packets, if needed, to hide number of packets
    let padding_size =
        MULTI_RECEIVED_CHANNEL_BLOOM_N.saturating_sub(received_packets.len() as u32) as usize;
    if padding_size > 0 {
        let padding_addresses = hkdf_sha_512(
            &Some(vec![0u8; 64]),
            &env_random,
            format!("{}:decoys", MULTI_RECEIVED_CHANNEL_ID).as_bytes(),
            padding_size * 20, // 20 bytes per random addr
        )?;

        // handle each padding package
        for i in 0..padding_size {
            let padding_address = &padding_addresses[i * 20..(i + 1) * 20];

            // contribute padding packet to bloom filter
            let seed = get_seed(&CanonicalAddr::from(padding_address), secret)?;
            let id = notification_id(&seed, &MULTI_RECEIVED_CHANNEL_ID.to_string(), &tx_hash)?;
            let hash_bytes = U256::from_big_endian(&sha_256(id.0.as_slice()));
            let bloom_mask: U256 = U256::from(0x1ff);
            for i in 0..MULTI_RECEIVED_CHANNEL_BLOOM_K {
                let bit_index = ((hash_bytes >> (256 - 9 - (i * 9))) & bloom_mask).as_usize();
                received_bloom_filter |= U512::from(1) << bit_index;
            }

            // padding packet plaintext
            let padding_packet_plaintext = [0u8; MULTI_RECEIVED_CHANNEL_PACKET_SIZE as usize];
            let padding_packet_id = &id.0.as_slice()[0..8];
            let padding_packet_ikm = &id.0.as_slice()[8..32];
            let padding_packet_ciphertext =
                xor_bytes(padding_packet_plaintext.as_slice(), padding_packet_ikm);
            let padding_packet_bytes: Vec<u8> =
                [padding_packet_id.to_vec(), padding_packet_ciphertext].concat();
            received_packets.push(padding_packet_bytes);
        }
    }

    let mut received_bloom_filter_bytes: Vec<u8> = vec![];
    received_bloom_filter_bytes.extend_from_slice(&received_bloom_filter.to_big_endian());

    for packet in received_packets {
        received_bloom_filter_bytes.extend(packet.iter());
    }

    Ok(received_bloom_filter_bytes)
}

pub fn multi_spent_data(
    api: &dyn Api,
    notifications: Vec<Notification<SpentNotificationData>>,
    tx_hash: &String,
    env_random: Binary,
    secret: &[u8],
) -> StdResult<Vec<u8>> {
    let mut spent_bloom_filter: U512 = U512::from(0);
    let mut spent_packets: Vec<(Addr, Vec<u8>)> = vec![];

    // keep track of how often addresses might show up in packet data.
    // we need to remove any address that might show up more than once.
    let mut spent_counts: HashMap<Addr, u16> = HashMap::new();

    for notification in &notifications {
        spent_counts.insert(
            notification.notification_for.clone(),
            spent_counts
                .get(&notification.notification_for)
                .unwrap_or(&0u16)
                + 1,
        );

        // we can short circuit this if recipient count > 1, since we will throw out this packet
        // anyway, and address has already been added to bloom filter
        if *spent_counts.get(&notification.notification_for).unwrap() > 1 {
            continue;
        }

        let spender_addr_raw = api.addr_canonicalize(notification.notification_for.as_str())?;
        let seed = get_seed(&spender_addr_raw, secret)?;
        let id = notification_id(&seed, &MULTI_SPENT_CHANNEL_ID.to_string(), &tx_hash)?;
        let hash_bytes = U256::from_big_endian(&sha_256(id.0.as_slice()));
        let bloom_mask: U256 = U256::from(0x1ff);
        for i in 0..MULTI_RECEIVED_CHANNEL_BLOOM_K {
            let bit_index = ((hash_bytes >> (256 - 9 - (i * 9))) & bloom_mask).as_usize();
            spent_bloom_filter |= U512::from(1) << bit_index;
        }

        // make the spent packet
        let mut packet_plaintext = vec![0u8; MULTI_SPENT_CHANNEL_PACKET_SIZE];

        // amount bytes (u64 == 8 bytes)
        packet_plaintext.extend_from_slice(
            &notification.data.amount
                .clamp(0, u64::MAX as u128)
                .to_be_bytes()[8..]
        );

        // balance bytes (u64 == 8 bytes)
        packet_plaintext.extend_from_slice(
            &notification.data.amount
                .clamp(0, u64::MAX as u128)
                .to_be_bytes()[8..]
        );

        // recipient account last 8 bytes
        let recipient_bytes: &[u8];
        let recipient_raw;
        if let Some(recipient) = &notification.data.recipient {
            recipient_raw = api.addr_canonicalize(recipient.as_str())?;
            recipient_bytes = &recipient_raw.as_slice()[recipient_raw.0.len() - 8..];
        } else {
            recipient_bytes = &ZERO_ADDR[ZERO_ADDR.len() - 8..];
        }

        // 24 bytes total
        packet_plaintext.extend_from_slice(recipient_bytes);

        // let packet_key = hkdf_sha_512(
        //     &Some(vec![0u8; 64]),
        //     packet_ikm,
        //     "".as_bytes(),
        //     packet_size,
        // )?;
        
        // use top 64 bits of notification ID for packet ID
        let packet_id = &id.0.as_slice()[0..8];

        // take the bottom bits from the notification ID for key material
        let packet_ikm = &id.0.as_slice()[8..32];

        // create ciphertext by XOR'ing the plaintext with the notification ID
        let packet_ciphertext = xor_bytes(
            &packet_plaintext[..],
            &packet_ikm[0..MULTI_SPENT_CHANNEL_PACKET_SIZE]
        );

        // construct the packet bytes
        let packet_bytes: Vec<u8> = [
            packet_id.to_vec(),
            packet_ciphertext,
        ].concat();

        // add to packets data
        spent_packets.push((notification.notification_for.clone(), packet_bytes));
    }

    // filter out any notifications for senders showing up more than once
    let mut spent_packets: Vec<Vec<u8>> = spent_packets
        .into_iter()
        .filter(|(addr, _)| *spent_counts.get(addr).unwrap_or(&0u16) <= 1)
        .map(|(_, packet)| packet)
        .collect();
    if spent_packets.len() > MULTI_SPENT_CHANNEL_BLOOM_N {
        // still too many packets
        spent_packets = spent_packets[0..MULTI_SPENT_CHANNEL_BLOOM_N].to_vec();
    }

    // now add extra packets, if needed, to hide number of packets
    let padding_size = MULTI_SPENT_CHANNEL_BLOOM_N.saturating_sub(spent_packets.len());
    if padding_size > 0 {
        let padding_addresses = hkdf_sha_512(
            &Some(vec![0u8; 64]),
            &env_random,
            format!("{}:decoys", MULTI_SPENT_CHANNEL_ID).as_bytes(),
            padding_size * 20, // 20 bytes per random addr
        )?;

        // handle each padding package
        for i in 0..padding_size {
            let padding_address = &padding_addresses[i * 20..(i + 1) * 20];

            // contribute padding packet to bloom filter
            let seed = get_seed(&CanonicalAddr::from(padding_address), secret)?;
            let id = notification_id(&seed, &MULTI_SPENT_CHANNEL_ID.to_string(), &tx_hash)?;
            let hash_bytes = U256::from_big_endian(&sha_256(id.0.as_slice()));
            let bloom_mask: U256 = U256::from(0x1ff);
            for i in 0..MULTI_RECEIVED_CHANNEL_BLOOM_K {
                let bit_index = ((hash_bytes >> (256 - 9 - (i * 9))) & bloom_mask).as_usize();
                spent_bloom_filter |= (U512::from(1) << bit_index);
            }

            // padding packet plaintext
            let padding_packet_plaintext = [0u8; MULTI_SPENT_CHANNEL_PACKET_SIZE as usize];
            let padding_plaintext_size = MULTI_SPENT_CHANNEL_PACKET_SIZE as usize;
            let padding_packet_id = &id.0.as_slice()[0..8];
            let padding_packet_ikm = &id.0.as_slice()[8..32];
            let padding_packet_key = hkdf_sha_512(
                &Some(vec![0u8; 64]),
                padding_packet_ikm,
                "".as_bytes(),
                padding_plaintext_size,
            )?;
            let padding_packet_ciphertext = xor_bytes(
                padding_packet_plaintext.as_slice(),
                padding_packet_key.as_slice(),
            );
            let padding_packet_bytes: Vec<u8> =
                [padding_packet_id.to_vec(), padding_packet_ciphertext].concat();
            spent_packets.push(padding_packet_bytes);
        }
    }

    let mut spent_bloom_filter_bytes: Vec<u8> = vec![];
    spent_bloom_filter_bytes.extend_from_slice(&spent_bloom_filter.to_big_endian());

    for packet in spent_packets {
        spent_bloom_filter_bytes.extend(packet.iter());
    }

    Ok(spent_bloom_filter_bytes)
}