use std::path::PathBuf;

use folio::{Session, boot};

fn main() -> iced::Result {
    let session = match std::env::args().nth(1) {
        Some(path) => Session::open_path(PathBuf::from(path)),
        None => Session::empty(),
    };
    iced::application("Tsuro", Session::update, Session::view)
        .subscription(Session::subscription)
        .run_with(move || boot(session))
}
