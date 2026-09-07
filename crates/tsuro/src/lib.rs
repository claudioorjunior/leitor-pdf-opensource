pub mod browse;
pub mod page;
pub mod session;
pub mod view;

pub(crate) mod engine;

pub use page::{Bitmap, MediaBox, PageNo, Quad, Scale, TextLayer, Viewport};
pub use session::{Message, OpenSource, Session};

pub fn boot(session: Session) -> (Session, iced::Task<Message>) {
    session.boot()
}
