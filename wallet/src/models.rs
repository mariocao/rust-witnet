use diesel::{Insertable, Queryable};
use serde::Serialize;

use super::schema::wallets;

#[derive(Debug, Queryable, Serialize)]
pub struct Wallet {
    pub id: i32,
    pub database: String,
    pub name: Option<String>,
    pub caption: Option<String>,
}

#[derive(Debug, Insertable)]
#[table_name = "wallets"]
pub struct NewWallet<'a> {
    pub database: &'a str,
    pub name: Option<&'a str>,
    pub caption: Option<&'a str>,
}
