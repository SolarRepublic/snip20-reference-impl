use cosmwasm_std::{Binary, StdResult, Storage};
use secret_toolkit::storage::{Keyset, Keymap};
use serde::{Serialize, Deserialize};

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

//spent_tokens = [
//    amount: biguint,   ; transfer amount in base denomination
//    recipient: bstr,   ; byte sequence of recipient's canonical address
//    balance: biguint   ; sender's new balance after the transfer
//]

// id for the `spent_tokens` channel
pub const SPENT_TOKENS_CHANNEL_ID: &str = "spent_tokens";
// CDDL Schema for `spent_tokens` channel data
pub const SPENT_TOKENS_CHANNEL_SCHEMA: &str = "spent_tokens=[amount:biguint,recipient:bstr,balance:biguint]";

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