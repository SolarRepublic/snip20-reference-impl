use cosmwasm_std::{Storage, StdResult};
use secret_toolkit::storage::Item;

pub const PREFIX_BURN_BYTE: &[u8] = b"__burnbyte__";
pub static BURN_BYTE: Item<u8> = Item::new(PREFIX_BURN_BYTE);

pub fn evaporate_gas(store: &mut dyn Storage, multiplier: u32) -> StdResult<()> {
    for _ in 0..multiplier {
        //if u64::from(i) >= u64::MAX {
            // never executes but compiler will not optimize away for loop
            BURN_BYTE.save(store, &0_u8)?;
        //}
    }
    Ok(())
}