use std::ptr;
use cosmwasm_std::{Storage, StdResult, Api,};
use schemars::JsonSchema;
use secret_toolkit::storage::Item;
use serde::{Serialize, Deserialize};

pub const PREFIX_EVAPORATE_BYTE: &[u8] = b"__evaporatebyte__";
pub static EVAPORATE_BYTE: Item<u8> = Item::new(PREFIX_EVAPORATE_BYTE);

pub const WRITE_MEM: u8 = 0;
pub const WRITE_STORAGE_BYTE: u8 = 1;
pub const READ_STORAGE_BYTE: u8 = 2;
pub const CANONICALIZE_ADDR: u8 = 3;
pub const VALIDATE_ADDR: u8 = 4;
pub const SECP256K1_SIGN: u8 = 5;
pub const ED25519_SIGN: u8 = 6;

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct EvaporateParams {
    pub factor: Option<u32>,
    pub technique: Option<u8>,
}

pub fn evaporate_gas(
    store: &mut dyn Storage, 
    api: &dyn Api,
    evaporate: u32,
    technique: u8,
) -> StdResult<()> {
    // write byte in memory technique
    if technique == WRITE_MEM {
        let mut x = 0;
        let ptr_x = &mut x as *mut i32;

        unsafe {
            for _ in 0..evaporate {
                ptr::write_volatile(ptr_x, *ptr_x);
            }
        }
    }

    // write byte in storage technique
    if technique == WRITE_STORAGE_BYTE {
        for _ in 0..evaporate {
            EVAPORATE_BYTE.save(store, &0_u8)?;
        }
    }

    // read byte in storage technique
    if technique == READ_STORAGE_BYTE {
        EVAPORATE_BYTE.save(store, &0_u8)?;
        for _ in 0..evaporate {
            EVAPORATE_BYTE.load(store)?;
        }
    }

    // canonicalize address technique
    if technique == CANONICALIZE_ADDR {
        for _ in 0..evaporate {
            // sscrt mainnet addr
            api.addr_canonicalize("secret18txpukk4n4cvkyytvgfsv0eqmadv2kcag8ypag")?;
        }
    }

    // validate address technique
    if technique == VALIDATE_ADDR {
        for _ in 0..evaporate {
            // sscrt mainnet addr
            api.addr_validate("secret18txpukk4n4cvkyytvgfsv0eqmadv2kcag8ypag")?;
        }
    }

    // secp256k1 sign technique
    if technique == SECP256K1_SIGN {
        for _ in 0..evaporate {
            let _signature = api.secp256k1_sign("message".as_bytes(), "key".as_bytes());
        }
    }

    // ed25519 sign technique
    if technique == ED25519_SIGN {
        for _ in 0..evaporate {
            let _signature = api.ed25519_sign("message".as_bytes(), "key".as_bytes());
        }
    }

    Ok(())
}