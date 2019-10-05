use diesel::prelude::*;

use crate::models::*;
use crate::schema::wallets;

type Conn = SqliteConnection;

pub fn list(conn: &Conn) -> Result<Vec<Wallet>, failure::Error> {
    use crate::schema::wallets::dsl::*;

    let infos = wallets.load::<Wallet>(conn)?;

    Ok(infos)
}

pub fn create(
    conn: &Conn,
    database: &str,
    name: Option<&str>,
    caption: Option<&str>,
) -> Result<i32, failure::Error> {
    let new_wallet = NewWallet {
        database,
        name,
        caption,
    };

    let id = conn.transaction(|| {
        use crate::schema::wallets::dsl::*;

        diesel::insert_into(crate::schema::wallets::table)
            .values(&new_wallet)
            .execute(conn)?;

        wallets.select(id).order(id.desc()).first(conn)
    })?;

    println!("ID:::::: {}", id);

    Ok(id)
}
