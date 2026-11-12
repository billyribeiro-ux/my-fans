use crate::{Creator, FeedPost, Visibility};

pub fn creators() -> Vec<Creator> {
    [
        ("a1111111-1111-4111-8111-111111111111", "ellawild", "Ella Wild", "Travel", "Slow mornings, faraway places, and the stories in between. Come along for the adventure.", "photo-1534528741775-53994a69daeb", "photo-1464822759023-fed622ff2c3b", 1200, 24800),
        ("b2222222-2222-4222-8222-222222222222", "marcostable", "Marco's Table", "Food", "A seat at my table. Seasonal recipes, kitchen experiments, and a little everyday magic.", "photo-1500648767791-00dcc994a43e", "photo-1547592180-85f173990554", 900, 12600),
        ("c3333333-3333-4333-8333-333333333333", "studioluna", "Luna Chen", "Art", "Making things with my hands. Inside the studio: ceramics, sketchbooks, and creative rituals.", "photo-1524504388940-b1c1722653e1", "photo-1493106641515-6b5631de4bb9", 1500, 8700),
        ("d4444444-4444-4444-8444-444444444444", "movebyjames", "James Walker", "Fitness", "Movement that feels good. Practical training, mindful recovery, and showing up for yourself.", "photo-1506794778202-cad84cf45f1d", "photo-1517836357463-d25dfeac3438", 1800, 18200),
        ("e5555555-5555-4555-8555-555555555555", "sophiarose", "Sophia Rose", "Lifestyle", "Finding beauty in the everyday. Personal style, small spaces, and thoughtful living.", "photo-1524250502761-1ac6f2e30d43", "photo-1441986300917-64674bd600d8", 1000, 21300),
    ].into_iter().map(|(id, handle, name, category, bio, avatar, cover, price, count)| Creator {
        id: id.into(), handle: handle.into(), display_name: name.into(), category: category.into(), bio: bio.into(),
        avatar_url: format!("https://images.unsplash.com/{avatar}?w=160&h=160&fit=crop&crop=faces&auto=format"),
        cover_url: format!("https://images.unsplash.com/{cover}?w=1000&h=500&fit=crop&auto=format"),
        subscription_price_minor: price, currency: "USD".into(), subscriber_count: count, subscribed: false,
    }).collect()
}

pub fn posts() -> Vec<FeedPost> {
    [
        ("p1", "a1111111-1111-4111-8111-111111111111", "Some places make you forget to check your phone. A few moments from the Dolomites, just for you. 🌿", "2026-10-06T12:00:00Z", "photo-1464822759023-fed622ff2c3b", Visibility::Public, 1248, 86),
        ("p2", "b2222222-2222-4222-8222-222222222222", "Sunday, served slowly. My favorite handmade pasta recipe — plus the little trick that changes everything.", "2026-10-06T10:30:00Z", "photo-1473093295043-cdd812d0e601", Visibility::Public, 892, 42),
        ("p3", "a1111111-1111-4111-8111-111111111111", "The trail less traveled. My complete 5-day itinerary, hidden viewpoints, and where we stayed. Members, this one is for you.", "2026-10-05T16:00:00Z", "photo-1472396961693-142e6e269027", Visibility::Subscribers, 674, 38),
        ("p4", "c3333333-3333-4333-8333-333333333333", "A quiet afternoon in the studio. New forms, old favorites, and a reminder to trust the process.", "2026-10-05T14:00:00Z", "photo-1493106641515-6b5631de4bb9", Visibility::Public, 538, 27),
        ("p5", "d4444444-4444-4444-8444-444444444444", "Your next chapter starts with one good session. The full 4-week movement program is ready for members.", "2026-10-05T09:00:00Z", "photo-1517836357463-d25dfeac3438", Visibility::Subscribers, 421, 19),
    ].into_iter().map(|(id, creator_id, caption, date, image, visibility, likes, comments)| FeedPost {
        id: id.into(), creator_id: creator_id.into(), caption: caption.into(), created_at: date.into(),
        // These are public stock assets for the demo, not protected production media.
        media_url: Some(format!("https://images.unsplash.com/{image}?w=1200&h=850&fit=crop&auto=format")),
        preview_url: format!("https://images.unsplash.com/{image}?w=80&h=60&fit=crop&blur=40&auto=format"),
        visibility, locked: visibility == Visibility::Subscribers, like_count: likes, comment_count: comments,
    }).collect()
}
