use cosmwasm_std::{Storage, StdResult};
use secret_toolkit::storage::Item;

pub const PREFIX_EVAPORATE_BYTE: &[u8] = b"__evaporatebyte__";
pub static EVAPORATE_BYTE: Item<u8> = Item::new(PREFIX_EVAPORATE_BYTE);

pub fn evaporate_gas(store: &mut dyn Storage, multiplier: u32) -> StdResult<()> {
    for i in 0..multiplier {
        EVAPORATE_BYTE.save(store, &0_u8)?;
    }
    Ok(())
}