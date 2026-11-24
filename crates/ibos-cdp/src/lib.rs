//! Chrome DevTools Protocol client and browser supervisor.
//!
//! * [`Conn`] - WebSocket client. Every call has a timeout; a silent socket becomes an error, never a hang. Queues are bounded.
//! * [`Browser`] - launches Chrome/Chromium/Edge in its own process group (a job object on Windows) and kills the whole tree
//!   on drop, so a crash of the agent cannot leave a headless browser running.
//! * [`Page`] - navigation, observation (accessibility tree + layout), and actions addressed by stable backend node ids.

mod browser;
mod chrome;
mod conn;
mod error;
mod page;

pub use browser::{Browser, LaunchOptions};
pub use chrome::{find_browser, parse_devtools_url};
pub use conn::{Conn, Event};
pub use error::CdpError;
pub use page::{Page, RawObservation, Viewport};
