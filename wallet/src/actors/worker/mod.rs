use std::result;
use std::sync::Arc;

use actix::prelude::*;
use diesel::prelude::*;

use crate::{db, params, repository, types};

pub mod error;
pub mod handlers;
pub mod methods;

pub use error::*;
pub use handlers::*;

pub type Result<T> = result::Result<T, Error>;

pub struct Worker {
    db: Arc<rocksdb::DB>,
    new_db: diesel::r2d2::Pool<diesel::r2d2::ConnectionManager<SqliteConnection>>,
    wallets: Arc<repository::Wallets<db::PlainDb>>,
    params: params::Params,
    engine: types::SignEngine,
    rng: rand::rngs::OsRng,
}

impl Actor for Worker {
    type Context = SyncContext<Self>;
}
