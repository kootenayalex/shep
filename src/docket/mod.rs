//! The personal docket: recurring, slated and captured items the server keeps
//! for its owner (`docs/VISION.md`, the M4 retirement note). Pure date
//! arithmetic in [`dates`], the sqlite store in [`store`].

pub(crate) mod dates;
pub(crate) mod store;

pub(crate) use store::{
    add, complete, discard, docket_db_path, list, open_store, promote, update, DocketError,
    ItemPatch, NewItem,
};
