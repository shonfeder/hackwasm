use cosmwasm_std::{Addr, Timestamp, Uint128};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use cw_storage_plus::{Item, Map};

// Initialize a contract with the admin address and lotto id generator nonce
pub struct Config {
    pub manager_addr: Addr,
    pub lotto_nonce: u32,
    pub nois_proxy: Addr,
}

pub struct Lotto {
    pub nonce: u32,
    pub min_deposit: Uint128,
    pub deposit_amount: Uint128,
    pub depositors: Vec<Addr>,
    pub expiration: Timestamp, // how to set expiration
    pub winner: Option<Addr>,
}

pub const CONFIG_KEY: &str = "config";
pub const LOTTO_KEY: &str = "lottos";
pub const NOIS_KEY: &str = "nois_proxy";

pub const CONFIG: Item<Config> = Item::new(CONFIG_KEY);
pub const LOTTOS: Map<u32, Lotto> = Map::new(LOTTO_KEY);
pub const NOIS_PROXY: Item<Addr> = Item::new(NOIS_KEY);
