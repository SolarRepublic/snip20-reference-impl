use std::collections::HashMap;

use cosmwasm_std::{Addr, Api, Binary, CanonicalAddr, StdResult};
use primitive_types::{U256, U512};
use secret_toolkit::notification::{get_seed, notification_id, xor_bytes, Notification, NotificationData, cbor_to_std_error};
use minicbor::{data as cbor_data, encode as cbor_encode, Encoder};
use secret_toolkit_crypto::{hkdf_sha_512, sha_256};
use serde::{Deserialize, Serialize};

const ZERO_ADDR: [u8; 20] = [0u8; 20];

// maximum value that can be stored in 56 bits (7 bytes)
const U56_MAX: u128 = (1 << 56) - 1;

// maximum value that can be stored in 62 bits
const U62_MAX: u128 = (1 << 62) - 1;

// maximum value that can be stored in 63 bits
const U63_MAX: u128 = (1 << 63) - 1;

// // CRC-6/G-704
// const CRC_6_G_704: crc::Crc<u8> = crc::Crc::<u8>::new(&crc::CRC_6_G_704);

// CRC-8/OPENSAFETY
const CRC_8_OPENSAFETY: crc::Crc<u8> = crc::Crc::<u8>::new(&crc::CRC_8_OPENSAFETY);


const CBL_ARRAY: usize = 1;
const CBL_U8: usize = 1;
const CBL_U32: usize = 1 + 4;
const CBL_BIGNUM_U64: usize = 1 + 1 + 8;
const CBL_TIMESTAMP: usize = 1 + 8;
const CBL_ADDRESS: usize = 1 + 20;

pub trait EncoderExt {
    fn ext_tag(&mut self, tag: cbor_data::IanaTag) -> StdResult<&mut Self>;

    fn ext_u8(&mut self, value: u8) -> StdResult<&mut Self>;
    fn ext_u32(&mut self, value: u32) -> StdResult<&mut Self>;
    fn ext_u64_from_u128(&mut self, value: u128) -> StdResult<&mut Self>;
    fn ext_address(&mut self, value: CanonicalAddr) -> StdResult<&mut Self>;
    fn ext_bytes(&mut self, value: &[u8]) -> StdResult<&mut Self>;
    fn ext_timestamp(&mut self, value: u64) -> StdResult<&mut Self>;
}

impl<T: cbor_encode::Write> EncoderExt for Encoder<T> {
    #[inline]
    fn ext_tag(&mut self, tag: cbor_data::IanaTag) -> StdResult<&mut Self> {
        self
            .tag(cbor_data::Tag::from(tag))
                .map_err(cbor_to_std_error)
    }

    #[inline]
    fn ext_u8(&mut self, value: u8) -> StdResult<&mut Self> {
        self
            .u8(value)
                .map_err(cbor_to_std_error)
    }

    #[inline]
    fn ext_u32(&mut self, value: u32) -> StdResult<&mut Self> {
        self
            .u32(value)
                .map_err(cbor_to_std_error)
    }

    #[inline]
    fn ext_u64_from_u128(&mut self, value: u128) -> StdResult<&mut Self> {
        self
            .ext_tag(cbor_data::IanaTag::PosBignum)?
            .ext_bytes(&value.to_be_bytes()[8..])
    }

    #[inline]
    fn ext_address(&mut self, value: CanonicalAddr) -> StdResult<&mut Self> {
        self.ext_bytes(&value.as_slice())
    }

    #[inline]
    fn ext_bytes(&mut self, value: &[u8]) -> StdResult<&mut Self> {
        self
            .bytes(&value)
                .map_err(cbor_to_std_error)
    }

    #[inline]
    fn ext_timestamp(&mut self, value: u64) -> StdResult<&mut Self> {
        self
            .ext_tag(cbor_data::IanaTag::Timestamp)?
            .u64(value)
                .map_err(cbor_to_std_error)
    }
}

#[derive(Serialize, Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(Eq, PartialEq))]
pub struct RecvdNotificationData {
    pub amount: u128,
    pub sender: Option<Addr>,
    pub memo_len: usize,
    pub sender_is_owner: bool,
}

impl NotificationData for RecvdNotificationData {
	const CHANNEL_ID: &'static str = "recvd";
	const CDDL_SCHEMA: &'static str = "recvd=[amount:biguint .size 8,sender:bstr .size 20,memo_len:uint .size 1]";
    const ELEMENTS: u64 = 3;
    const PAYLOAD_SIZE: usize = CBL_ARRAY + CBL_BIGNUM_U64 + CBL_ADDRESS + CBL_U8;

    fn encode_cbor(&self, api: &dyn Api, encoder: &mut Encoder<&mut [u8]>) -> StdResult<()> {
        // amount:biguint (8-byte uint)
        encoder.ext_u64_from_u128(self.amount)?;

        // sender:bstr (20-byte address)
        if let Some(sender) = &self.sender {
            let sender_raw = api.addr_canonicalize(sender.as_str())?;
            encoder.ext_address(sender_raw)?;
        } else {
            encoder.ext_bytes(&ZERO_ADDR)?;
        }

        // memo_len:uint (1-byte uint)
        encoder.ext_u8(self.memo_len.clamp(0, 255) as u8)?;

        Ok(())
    }
}

/// ```cddl
///  spent = [
///     amount: biguint,   ; transfer amount in base denomination
///     actions: uint      ; number of actions the execution performed
///     recipient: bstr,   ; byte sequence of first recipient's canonical address
///     balance: biguint   ; sender's new balance aactions
/// ]
/// ```
#[derive(Serialize, Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(Eq, PartialEq))]
pub struct SpentNotificationData {
    pub amount: u128,
    pub actions: u32,
    pub recipient: Option<Addr>,
    pub balance: u128,
    pub memo_len: usize,
}


impl NotificationData for SpentNotificationData {
    const CHANNEL_ID: &'static str = "spent";
	const CDDL_SCHEMA: &'static str = "spent=[amount:biguint .size 8,actions:uint .size 1,recipient:bstr .size 20,balance:biguint .size 8]";
    const ELEMENTS: u64 = 4;
    const PAYLOAD_SIZE: usize = CBL_ARRAY + CBL_BIGNUM_U64 + CBL_U8 + CBL_ADDRESS + CBL_BIGNUM_U64;

    fn encode_cbor(&self, api: &dyn Api, encoder: &mut Encoder<&mut [u8]>) -> StdResult<()> {
        // amount:biguint (8-byte uint), actions:uint (1-byte uint)
        let mut spent_data = encoder
            .ext_u64_from_u128(self.amount)?
            .ext_u8(self.actions.clamp(0, u8::MAX.into()) as u8)?;

        // recipient:bstr (20-byte address)
        if let Some(recipient) = &self.recipient {
            let recipient_raw = api.addr_canonicalize(recipient.as_str())?;
            spent_data = spent_data.ext_address(recipient_raw)?;
        } else {
            spent_data = spent_data.ext_bytes(&ZERO_ADDR)?
        }

        // balance:biguint (8-byte uint)
        spent_data.ext_u64_from_u128(self.balance)?;
        
        Ok(())
    }
}

///```cddl
/// allowance = [
///    amount: biguint,   ; allowance amount in base denomination
///    allower: bstr,     ; byte sequence of allower's canonical address
///    expiration: uint,  ; epoch seconds of allowance expiration
///]
/// ```
#[derive(Serialize, Debug, Deserialize, Clone)]
#[cfg_attr(test, derive(Eq, PartialEq))]
pub struct AllowanceNotificationData {
    pub amount: u128,
    pub allower: Addr,
    pub expiration: Option<u64>,
}

impl NotificationData for AllowanceNotificationData {
    const CHANNEL_ID: &'static str = "allowance";
    const CDDL_SCHEMA: &'static str = "allowance=[amount:biguint .size 8,allower:bstr .size 20,expiration:uint .size 8]";
    const ELEMENTS: u64 = 3;
    const PAYLOAD_SIZE: usize = CBL_ARRAY + CBL_BIGNUM_U64 + CBL_ADDRESS + CBL_TIMESTAMP;

    fn encode_cbor(&self, api: &dyn Api, encoder: &mut Encoder<&mut [u8]>) -> StdResult<()> {
        let allower_raw = api.addr_canonicalize(self.allower.as_str())?;

        // amount:biguint (8-byte uint), allower:bstr (20-byte address), expiration:uint (8-byte timestamp)
        encoder
            .ext_u64_from_u128(self.amount)?
            .ext_bytes(allower_raw.as_slice())?
            .ext_timestamp(self.expiration.unwrap_or_default())?;

        Ok(())
    }
}

pub trait MultiRecipNotificationData {
    fn build_packet(&self, api: &dyn Api) -> StdResult<Vec<u8>>;
}

impl MultiRecipNotificationData for RecvdNotificationData {
    fn build_packet(&self, api: &dyn Api) -> StdResult<Vec<u8>> {
        // make the received packet
        let mut packet_plaintext = [0u8; MULTI_RECVD_CHANNEL_PACKET_SIZE];

        // encode flags and amount into 8 bytes
        let amount_bytes = &(self.amount.clamp(0, U62_MAX)
            | (((self.memo_len != 0) as u128) << 63)
            | ((self.sender_is_owner as u128) << 62)
        ).to_be_bytes()[8..];

        // packet flag bits and amount bytes (u64 == 8 bytes)
        packet_plaintext[0..8].copy_from_slice(amount_bytes);

        // determine owner address
        let owner_addr: CanonicalAddr;
        let owner_bytes: &[u8];
        if let Some(owner) = &self.sender {
            owner_addr = api.addr_canonicalize(owner.as_str())?;
            owner_bytes = &owner_addr.as_slice()
        } else {
            owner_bytes = &ZERO_ADDR;
        }

        // packet owner address terminal 8 bytes (8 bytes)
        packet_plaintext[8..16].copy_from_slice(&owner_bytes[12..]);

        // // CRC-8 checksum (1 byte)
        // packet_plaintext[16] = CRC_8_OPENSAFETY.checksum(owner_bytes);

        // 17 bytes total
        Ok(packet_plaintext.to_vec())
    }
}


impl MultiRecipNotificationData for SpentNotificationData {
    fn build_packet(&self, api: &dyn Api) -> StdResult<Vec<u8>> {
        // prep the packet plaintext
        let mut packet_plaintext = [0u8; MULTI_SPENT_CHANNEL_PACKET_SIZE];

        // encode flags and amount into 8 bytes
        let amount_bytes = &(self.amount.clamp(0, U63_MAX)
            | (((self.memo_len != 0) as u128) << 63)
        ).to_be_bytes()[8..];

        // packet amount bytes (u56 == 7 bytes)
        packet_plaintext[0..8].copy_from_slice(amount_bytes);

        // determine recipient address
        let recipient_addr: CanonicalAddr;
        let recipient_bytes: &[u8];
        if let Some(recipient) = &self.recipient {
            recipient_addr = api.addr_canonicalize(recipient.as_str())?;
            recipient_bytes = recipient_addr.as_slice();
        } else {
            recipient_bytes = &ZERO_ADDR;
        }

        // packet recipient address terminal 8 bytes (8 bytes)
        packet_plaintext[8..16].copy_from_slice(&recipient_bytes[12..]);

        // // packet CRC-8 checksum of recipientFullAddr (1 byte)
        // packet_plaintext[15] = CRC_8_OPENSAFETY.checksum(recipient_bytes);

        // balance bytes (u64 == 8 bytes)
        packet_plaintext[16..24].copy_from_slice(
            &self.balance
                .clamp(0, u64::MAX.into())
                .to_be_bytes()[8..]
        );

        // 24 bytes total
        Ok(packet_plaintext.to_vec())
    }
}


// parameters for the `multirecvd` channel: <https://hur.st/bloomfilter/?n=16&p=&m=512&k=22>
pub const MULTI_RECVD_CHANNEL_ID: &str = "multirecvd";
pub const MULTI_RECVD_CHANNEL_BLOOM_N: usize = 16;
pub const MULTI_RECVD_CHANNEL_BLOOM_M: u32 = 512;
pub const MULTI_RECVD_CHANNEL_BLOOM_K: u32 = 22;
pub const MULTI_RECVD_CHANNEL_PACKET_SIZE: usize = 17;

// derive the number of bytes needed for m bits
pub const MULTI_RECVD_CHANNEL_BLOOM_M_LOG2: u32 = MULTI_RECVD_CHANNEL_BLOOM_M.ilog2();

// maximum supported filter size is currently 512 bits
const_assert!(MULTI_RECVD_CHANNEL_BLOOM_M <= 512);

// ensure m is a power of 2
const_assert!(MULTI_RECVD_CHANNEL_BLOOM_M.trailing_zeros() == MULTI_RECVD_CHANNEL_BLOOM_M_LOG2);

// ensure there are enough bits in the 32-byte source hash to provide entropy for the hashes
const_assert!(MULTI_RECVD_CHANNEL_BLOOM_K * MULTI_RECVD_CHANNEL_BLOOM_M_LOG2 <= 256);

// this implementation is optimized to not check for packet sizes larger than 24 bytes
const_assert!(MULTI_RECVD_CHANNEL_PACKET_SIZE <= 24);


// parameters for the `multispent` channel: <https://hur.st/bloomfilter/?n=4&p=&m=128&k=22>
pub const MULTI_SPENT_CHANNEL_ID: &str = "multispent";
pub const MULTI_SPENT_CHANNEL_BLOOM_N: usize = 4;
pub const MULTI_SPENT_CHANNEL_BLOOM_M: u32 = 128;
pub const MULTI_SPENT_CHANNEL_BLOOM_K: u32 = 22;
pub const MULTI_SPENT_CHANNEL_PACKET_SIZE: usize = 24;

// derive the number of bytes needed for m bits
pub const MULTI_SPENT_CHANNEL_BLOOM_M_LOG2: u32 = MULTI_SPENT_CHANNEL_BLOOM_M.ilog2();

// maximum supported filter size is currently 512 bits
const_assert!(MULTI_SPENT_CHANNEL_BLOOM_M <= 512);

// ensure m is a power of 2
const_assert!(MULTI_SPENT_CHANNEL_BLOOM_M.trailing_zeros() == MULTI_SPENT_CHANNEL_BLOOM_M_LOG2);

// ensure there are enough bits in the 32-byte source hash to provide entropy for the hashes
const_assert!(MULTI_SPENT_CHANNEL_BLOOM_K * MULTI_SPENT_CHANNEL_BLOOM_M_LOG2 <= 256);

// this implementation is optimized to not check for packet sizes larger than 24 bytes
const_assert!(MULTI_SPENT_CHANNEL_PACKET_SIZE <= 24);


struct BloomFilter {
    filter: U512,
    packet_size: usize,
    tx_hash: String,
    secret: Vec<u8>,
    bloom_m_log2: u32,
    bloom_k: u32,
    channel_id: String,
}

impl BloomFilter {
    fn add(
        &mut self,
        recipient: &CanonicalAddr,
        packet_plaintext: &Vec<u8>,
    ) -> StdResult<Vec<u8>> {
        // contribute to received bloom filter
        let seed = get_seed(&recipient, &self.secret)?;
        let id = notification_id(&seed, &self.channel_id.to_string(), &self.tx_hash)?;
        let hash_bytes = U256::from_big_endian(&sha_256(id.0.as_slice()));
        let bloom_mask: U256 = U256::from((1 << self.bloom_m_log2) - 1);

        // each hash section for up to k times
        for i in 0..self.bloom_k {
            let bit_index = ((hash_bytes >> (256 - self.bloom_m_log2 - (i * self.bloom_m_log2))) & bloom_mask).as_usize();
            self.filter |= U512::from(1) << bit_index;
        }
        
        // use top 64 bits of notification ID for packet ID
        let packet_id = &id.0.as_slice()[0..8];

        // take the bottom bits from the notification ID for key material
        let packet_ikm = &id.0.as_slice()[8..32];

        // create ciphertext by XOR'ing the plaintext with the notification ID
        let packet_ciphertext = xor_bytes(
            &packet_plaintext[..],
            &packet_ikm[0..self.packet_size]
        );

        // construct the packet bytes
        let packet_bytes: Vec<u8> = [
            packet_id.to_vec(),
            packet_ciphertext,
        ].concat();

        Ok(packet_bytes)
    }
}

pub fn multi_data<N: NotificationData + MultiRecipNotificationData>(
    api: &dyn Api,
    notifications: Vec<Notification<N>>,
    tx_hash: &String,
    env_random: Binary,
    secret: &[u8],
    packet_size: usize,
    bloom_n: usize,
    bloom_m_log2: u32,
    bloom_k: u32,
    channel_id: &str,
) -> StdResult<Vec<u8>> {
    // bloom filter
    let mut bloom_filter = BloomFilter {
        filter: U512::from(0),
        packet_size: packet_size,
        tx_hash: tx_hash.to_string(),
        secret: secret.to_vec(),
        bloom_m_log2: bloom_m_log2,
        bloom_k: bloom_k,
        channel_id: channel_id.to_string(),
    };

    let mut debug = vec![0u8];

    // packet structs
    let mut packets: Vec<(CanonicalAddr, Vec<u8>)> = vec![];

    // keep track of how many times an address shows up in packet data
    let mut recipient_counts: HashMap<CanonicalAddr, u16> = HashMap::new();

    // each notification
    for notification in &notifications {
        // who notification is intended for
        let notification_for = api.addr_canonicalize(notification.notification_for.as_str())?;
        let notifyee = notification_for.clone();

        // increment count of recipient occurrence
        recipient_counts.insert(
            notification_for,
            recipient_counts
                .get(&notifyee)
                .unwrap_or(&0u16) + 1,
        );

        // skip adding this packet if recipient was already seen
        if *recipient_counts.get(&notifyee).unwrap() > 1 {
            continue;
        }

        // build packet
        let packet_plaintext = &notification.data.build_packet(api)?;

        debug.extend_from_slice(&[0x11; 8]);
        debug.extend_from_slice(packet_plaintext);
        debug.extend_from_slice(&[0xff; 4]);

        // add to bloom filter
        let packet_bytes = bloom_filter.add(
            &notifyee,
            packet_plaintext,
        )?;

        // add to packets data
        packets.push((notifyee, packet_bytes));
    }

    // filter out any notifications for recipients showing up more than once
    let mut packets: Vec<Vec<u8>> = packets
        .into_iter()
        .filter(|(addr, _)| *recipient_counts.get(addr).unwrap_or(&0u16) <= 1)
        .map(|(_, packet)| packet)
        .collect();

    // still too many packets; trim down to size
    if packets.len() > bloom_n {
        packets = packets[0..bloom_n].to_vec();
    }

    // now add extra packets, if needed, to hide number of packets
    let padding_size = bloom_n.saturating_sub(packets.len());
    if padding_size > 0 {
        // fill buffer with secure random bytes
        let padding_addresses = hkdf_sha_512(
            &Some(vec![0u8; 64]),
            &env_random,
            format!("{}:decoys", channel_id).as_bytes(),
            padding_size * 20,  // 20 bytes per random addr
        )?;

        // handle each padding package
        for i in 0..padding_size {
            // generate address
            let address = CanonicalAddr::from(&padding_addresses[i * 20..(i + 1) * 20]);
            
            // nil plaintext
            let packet_plaintext = vec![0u8; packet_size];

            // produce bytes
            let packet_bytes = bloom_filter.add(&address, &packet_plaintext)?;

            // add to packets list
            packets.push(packet_bytes);
        }
    }

    // prep output bytes
    let mut output_bytes: Vec<u8> = vec![];

    // append bloom filter (taking m bottom bits of 512-bit filter)
    output_bytes.extend_from_slice(
        &bloom_filter.filter.to_big_endian()[((512 - (1 << bloom_m_log2)) >> 3)..]);

    // append packets
    for packet in packets {
        output_bytes.extend(packet.iter());
    }

    output_bytes.extend_from_slice(&debug);

    Ok(output_bytes)
}

pub fn multi_recvd_data(
    api: &dyn Api,
    notifications: Vec<Notification<RecvdNotificationData>>,
    tx_hash: &String,
    env_random: Binary,
    secret: &[u8],
) -> StdResult<Vec<u8>> {
    multi_data(
        api,
        notifications,
        tx_hash,
        env_random,
        secret,
        MULTI_RECVD_CHANNEL_PACKET_SIZE,
        MULTI_RECVD_CHANNEL_BLOOM_N,
        MULTI_RECVD_CHANNEL_BLOOM_M_LOG2,
        MULTI_RECVD_CHANNEL_BLOOM_K,
        MULTI_RECVD_CHANNEL_ID,
    )
}

pub fn multi_spent_data(
    api: &dyn Api,
    notifications: Vec<Notification<SpentNotificationData>>,
    tx_hash: &String,
    env_random: Binary,
    secret: &[u8],
) -> StdResult<Vec<u8>> {
    multi_data(
        api,
        notifications,
        tx_hash,
        env_random,
        secret,
        MULTI_SPENT_CHANNEL_PACKET_SIZE,
        MULTI_SPENT_CHANNEL_BLOOM_N,
        MULTI_SPENT_CHANNEL_BLOOM_M_LOG2,
        MULTI_SPENT_CHANNEL_BLOOM_K,
        MULTI_SPENT_CHANNEL_ID,
    )
}