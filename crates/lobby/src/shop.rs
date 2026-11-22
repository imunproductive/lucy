//! The cosmetic shop shared by the game lobby and the dashboard.
//!
//! Ports `ow174/services/shop.py`. Prices come from the retail catalog's +0x14
//! field (+0x10 is an unlock level, not a price). OWL items cost league
//! tokens, golden weapons cost competitive points, everything else costs
//! credits. An Overwatch League team skin comes with its home or away partner.
//! The caller saves the profile and tells the client.

use indexmap::IndexMap;
use serde_json::Value as Json;

use crate::{accounts::Profile, collection::Collection};
use lucy_content::catalog::Unlock;

pub const PURCHASE: u32 = 24203;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Currency {
    Credits,
    LeagueTokens,
    CompPoints,
}

impl Currency {
    fn of(unlock: &Unlock) -> Self {
        if unlock.categories.iter().any(|category| category == "OWL") {
            Self::LeagueTokens
        } else if unlock.item_type == "WeaponSkin" {
            Self::CompPoints
        } else {
            Self::Credits
        }
    }

    /// The profile field that holds the balance.
    pub fn name(self) -> &'static str {
        match self {
            Self::Credits => "credits",
            Self::LeagueTokens => "league_tokens",
            Self::CompPoints => "comp_points",
        }
    }

    pub fn balance(self, profile: &mut Profile) -> &mut i64 {
        match self {
            Self::Credits => &mut profile.credits,
            Self::LeagueTokens => &mut profile.league_tokens,
            Self::CompPoints => &mut profile.comp_points,
        }
    }
}

/// The shop's offer for an item.
#[derive(Clone, Debug, PartialEq)]
pub struct Product {
    pub guid: u64,
    pub price: i64,
    pub currency: Currency,
}

/// A refused purchase: a stable code, the message and the HTTP status the
/// dashboard answers with.
#[derive(Debug, PartialEq, Eq)]
pub struct ShopError {
    pub code: &'static str,
    pub message: &'static str,
    pub status: u16,
}

const fn refused(code: &'static str, message: &'static str, status: u16) -> ShopError {
    ShopError {
        code,
        message,
        status,
    }
}

pub const INVALID_INPUT: ShopError = refused("invalid_input", "Invalid item identifier.", 400);

#[derive(Debug, PartialEq)]
pub struct Receipt {
    pub guid: u64,
    pub price: i64,
    pub currency: Currency,
    /// What came with it: the other skin of a team skin pair.
    pub also: Vec<u64>,
}

pub struct Shop {
    products: IndexMap<u64, Product>,
}

impl Shop {
    pub fn new(collection: &Collection) -> Self {
        let products = collection
            .store()
            .iter()
            .filter_map(|(&guid, entry)| {
                let unlock = collection.items().get(guid)?;
                (entry.price > 0).then(|| {
                    (
                        guid,
                        Product {
                            guid,
                            price: entry.price,
                            currency: Currency::of(unlock),
                        },
                    )
                })
            })
            .collect();
        Self { products }
    }

    pub fn product(&self, guid: u64) -> Option<&Product> {
        self.products.get(&guid)
    }

    /// Check the product, charge its price and add it (and a team skin's
    /// partner) to the profile's unlocks.
    pub fn purchase(
        &self,
        profile: &mut Profile,
        guid: u64,
        collection: &Collection,
    ) -> Result<Receipt, ShopError> {
        if guid == 0 {
            return Err(INVALID_INPUT);
        }
        let product = self.products.get(&guid).ok_or(refused(
            "not_purchasable",
            "This item is not available for purchase.",
            400,
        ))?;
        if profile.unlock_all || collection.owns(profile, guid) {
            return Err(refused("already_owned", "You already have this item.", 409));
        }
        let balance = product.currency.balance(profile);
        if *balance < product.price {
            return Err(refused("insufficient_balance", "Not enough funds.", 409));
        }
        *balance -= product.price;
        let also: Vec<u64> = collection
            .items()
            .team_skin_pair(guid)
            .filter(|&pair| !collection.owns(profile, pair))
            .into_iter()
            .collect();
        profile
            .unlocked_items
            .extend([guid].iter().chain(&also).map(|&guid| guid_text(guid)));
        Ok(Receipt {
            guid,
            price: product.price,
            currency: product.currency,
            also,
        })
    }

    /// Give the partners of owned team skins bought before the pairs came with
    /// them. Returns the added items.
    pub fn add_missing_pairs(profile: &mut Profile, collection: &Collection) -> Vec<u64> {
        if profile.unlock_all {
            return Vec::new();
        }
        let mut owned = collection.owned_set(profile);
        let mut unlocked: Vec<u64> = profile.unlocked_guids().into_iter().collect();
        unlocked.sort_unstable();
        let mut added = Vec::new();
        for guid in unlocked {
            if let Some(pair) = collection.items().team_skin_pair(guid) {
                if owned.insert(pair) {
                    added.push(pair);
                }
            }
        }
        profile
            .unlocked_items
            .extend(added.iter().map(|&guid| guid_text(guid)));
        added
    }
}

fn guid_text(guid: u64) -> Json {
    Json::String(format!("0x{guid:016X}"))
}

#[cfg(test)]
mod tests;
