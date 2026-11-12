pub mod platform;
mod seed;
use serde::Serialize;

#[derive(Clone, Serialize)]
pub struct Creator {
    pub id: String,
    pub handle: String,
    pub display_name: String,
    pub category: String,
    pub bio: String,
    pub avatar_url: String,
    pub cover_url: String,
    pub subscription_price_minor: u64,
    pub currency: String,
    pub subscriber_count: u64,
    pub subscribed: bool,
}

#[derive(Clone, Serialize)]
pub struct FeedPost {
    pub id: String,
    pub creator_id: String,
    pub caption: String,
    pub created_at: String,
    pub media_url: Option<String>,
    pub preview_url: String,
    pub visibility: Visibility,
    pub locked: bool,
    pub like_count: u64,
    pub comment_count: u64,
}

#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Public,
    Subscribers,
}
