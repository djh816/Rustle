use anyhow::{Context, Result};
use base64::Engine;
use eframe::egui;
use egui_extras::install_image_loaders;
use reqwest::{Client, header};
use serde::{Deserialize, Serialize};
use std::{
    sync::{Arc, Mutex},
    thread,
};
use keyring::Entry;

// Constants for application
const APP_NAME: &str = "Rustle";
const APP_VERSION: &str = "v0.1.0";
const APP_USER_AGENT: &str = concat!("Rustle:", env!("CARGO_PKG_VERSION"), " (by /u/SpartanJubilee)");

// API response models
#[derive(Debug, Deserialize)]
struct AuthResponse {
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct RedditListing {
    data: ListingData,
}

#[derive(Debug, Deserialize)]
struct ListingData {
    children: Vec<PostChild>,
    after: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PostChild {
    data: Post,
}

#[derive(Debug, Deserialize)]
struct Post {
    title: String,
    author: String,
    subreddit: String,
    score: i32,
    url: String,
    thumbnail: String,
    preview: Option<Preview>,
}

#[derive(Debug, Deserialize)]
struct Preview {
    images: Vec<Image>,
}

#[derive(Debug, Deserialize)]
struct Image {
    source: ImageSource,
    resolutions: Vec<ImageSource>,
}

#[derive(Debug, Deserialize)]
struct ImageSource {
    url: String,
    height: u32,
}

// New structs for subreddit data
#[derive(Debug, Deserialize)]
struct SubredditListing {
    data: SubredditListingData,
}

#[derive(Debug, Deserialize)]
struct SubredditListingData {
    children: Vec<SubredditChild>,
}

#[derive(Debug, Deserialize)]
struct SubredditChild {
    data: SubredditData,
}

#[derive(Debug, Deserialize)]
struct SubredditData {
    display_name: String,  // This is the subreddit name without the /r/ prefix
}

// Reddit API client
#[derive(Clone)]
struct RedditClient {
    client: Client,
    access_token: Option<String>,
}

impl RedditClient {
    fn new() -> Result<Self> {
        Ok(RedditClient {
            client: Client::builder()
                .user_agent(APP_USER_AGENT)
                .build()?,
            access_token: None,
        })
    }

    async fn authenticate(&mut self, client_id: &str, client_secret: &str, username: &str, password: &str) -> Result<()> {
        let auth = base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", client_id, client_secret));
        
        // Create a more reusable header builder
        let response = self.client
            .post("https://www.reddit.com/api/v1/access_token")
            .header(header::AUTHORIZATION, format!("Basic {}", auth))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .form(&[
                ("grant_type", "password"),
                ("username", username),
                ("password", password),
            ])
            .send()
            .await?;

        // Error handling
        let status = response.status();
        let response_text = response.text().await?;

        if !status.is_success() {
        if let Ok(error_response) = serde_json::from_str::<serde_json::Value>(&response_text) {
                if let Some(error) = error_response.get("error").and_then(|e| e.as_str()) {
                    return Err(anyhow::anyhow!("Reddit API error: {}", error));
                }
            }
            return Err(anyhow::anyhow!("Authentication failed with status: {}", status));
        }

        // Parse successful response
        let auth_response: AuthResponse = serde_json::from_str(&response_text)
            .context("Failed to parse authentication response")?;
            
        self.access_token = Some(auth_response.access_token);
        Ok(())
    }

    async fn get_home_feed(&self, after: Option<&str>) -> Result<(Vec<Post>, Option<String>)> {
        let access_token = self.access_token.as_ref()
            .context("Not authenticated")?;

        let mut url = "https://oauth.reddit.com/".to_string();
        if let Some(after_token) = after {
            url = format!("{}?after={}", url, after_token);
        }

        let response = self.client
            .get(&url)
            .header(header::AUTHORIZATION, format!("Bearer {}", access_token))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!("Failed to fetch home feed: {}", response.status()));
        }

        let listing: RedditListing = response.json().await
            .context("Failed to parse Reddit listing")?;
            
        Ok((listing.data.children.into_iter().map(|child| child.data).collect(), listing.data.after))
    }

    async fn get_subreddit_posts(&self, subreddit: &str, after: Option<&str>) -> Result<(Vec<Post>, Option<String>)> {
        let access_token = self.access_token.as_ref()
            .context("Not authenticated")?;

        let mut url = format!("https://oauth.reddit.com/r/{}", subreddit);
        if let Some(after_token) = after {
            url = format!("{}?after={}", url, after_token);
        }

        let response = self.client
            .get(&url)
            .header(header::AUTHORIZATION, format!("Bearer {}", access_token))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!("Failed to fetch subreddit posts: {}", response.status()));
        }

        let listing: RedditListing = response.json().await
            .context("Failed to parse Reddit listing")?;
            
        Ok((listing.data.children.into_iter().map(|child| child.data).collect(), listing.data.after))
    }

    async fn get_subscribed_subreddits(&self) -> Result<Vec<String>> {
        let access_token = self.access_token.as_ref()
            .context("Not authenticated")?;

        let url = "https://oauth.reddit.com/subreddits/mine/subscriber";

        let response = self.client
            .get(url)
            .header(header::AUTHORIZATION, format!("Bearer {}", access_token))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!("Failed to fetch subscribed subreddits: {}", response.status()));
        }

        let listing: SubredditListing = response.json().await
            .context("Failed to parse subreddits listing")?;
            
        Ok(listing.data.children.into_iter()
            .map(|child| child.data.display_name)
            .collect())
    }
}

// App state and UI
struct RedditApp {
    posts: Arc<Mutex<Vec<Post>>>,
    loading: Arc<Mutex<bool>>,
    error_message: Arc<Mutex<Option<String>>>,
    reddit_client: Arc<Mutex<Option<RedditClient>>>,
    after: Arc<Mutex<Option<String>>>,
    initial_load: Arc<Mutex<bool>>,
    scroll_to_top: Arc<Mutex<bool>>,
    show_settings: bool,
    settings: Settings,
    settings_modified: bool,
    has_credentials: bool,
    current_subreddit: Arc<Mutex<String>>,  // "home" for home feed, or subreddit name
    subreddits: Arc<Mutex<Vec<String>>>,    // List of user's subscribed subreddits
    loading_subreddits: Arc<Mutex<bool>>,   // Whether we're currently loading the subreddit list
    last_scroll_pos: Arc<Mutex<f32>>,       // Track the last scroll position
    is_loading_more: Arc<Mutex<bool>>,      // Track if we're in the process of loading more posts
}

#[derive(Clone, Serialize, Deserialize)]
struct Settings {
    client_id: String,
    client_secret: String,
    username: String,
    password: String,
    dark_mode: bool,  // Add theme preference
}

impl Settings {
    fn load() -> Self {
        let keyring = Entry::new("Rustle", "credentials").unwrap();
        let stored = keyring.get_password().unwrap_or_default();
        if !stored.is_empty() {
            if let Ok(settings) = serde_json::from_str(&stored) {
                return settings;
            }
        }
        
        // Default empty settings with dark mode enabled by default
        Settings {
            client_id: String::new(),
            client_secret: String::new(),
            username: String::new(),
            password: String::new(),
            dark_mode: true,  // Default to dark mode
        }
    }

    fn save(&self) -> Result<()> {
        let keyring = Entry::new("Rustle", "credentials")?;
        let json = serde_json::to_string(self)?;
        keyring.set_password(&json)?;
        Ok(())
    }
}

impl RedditApp {
    fn new() -> Self {
        let settings = Settings::load();
        let has_credentials = !settings.client_id.is_empty() 
            && !settings.client_secret.is_empty()
            && !settings.username.is_empty()
            && !settings.password.is_empty();

        Self { 
            posts: Arc::new(Mutex::new(Vec::new())),
            loading: Arc::new(Mutex::new(has_credentials)),  // Start loading if we have credentials
            error_message: Arc::new(Mutex::new(None)),
            reddit_client: Arc::new(Mutex::new(None)),
            after: Arc::new(Mutex::new(None)),
            initial_load: Arc::new(Mutex::new(has_credentials)),  // Show initial load if we have credentials
            scroll_to_top: Arc::new(Mutex::new(true)),  // Always start at top on fresh launch
            show_settings: !has_credentials,  // Show settings if no credentials
            settings,
            settings_modified: false,
            has_credentials,
            current_subreddit: Arc::new(Mutex::new("home".to_string())),
            subreddits: Arc::new(Mutex::new(Vec::new())),
            loading_subreddits: Arc::new(Mutex::new(false)),
            last_scroll_pos: Arc::new(Mutex::new(0.0)),
            is_loading_more: Arc::new(Mutex::new(false)),
        }
    }
    
    fn render_post(&self, ui: &mut egui::Ui, post: &Post) {
        ui.add_space(10.0);
        egui::Frame::group(ui.style())
            .fill(if self.settings.dark_mode {
                egui::Color32::from_rgb(20, 20, 20)
            } else {
                egui::Color32::from_rgb(240, 240, 240)
            })
            .outer_margin(0.0)  // Remove outer margin
            .show(ui, |ui| {
                // Use the full width
                ui.set_min_width(ui.available_width());
                
                ui.horizontal(|ui| {
                    // Find the resolution closest to our target size (100px)
                    let target_height = 100.0;
                    let image_url = post.preview.as_ref()
                        .and_then(|preview| preview.images.first())
                        .and_then(|image| {
                            image.resolutions.iter()
                                .min_by_key(|res| {
                                    // Calculate distance from target height
                                    ((res.height as f32 - target_height).abs() * 100.0) as i32
                                })
                                .or_else(|| image.resolutions.first())
                                .or(Some(&image.source))
                        })
                        .map(|img| img.url.replace("&amp;", "&"))
                        .unwrap_or_else(|| post.thumbnail.clone());

                    if image_url.starts_with("http") {
                        ui.add_space(5.0);
                        let image = egui::widgets::Image::new(image_url)
                            .fit_to_original_size(1.0)
                            .max_size(egui::Vec2::new(100.0, 100.0));
                        ui.add(image);
                        ui.add_space(10.0);
                    }

                    ui.vertical(|ui| {
                        // Make the vertical content take remaining width
                        ui.set_min_width(ui.available_width());
                        
                        // Post title with link
                        ui.add(
                            egui::Hyperlink::from_label_and_url(
                                egui::RichText::new(&post.title)
                                    .size(16.0)
                                    .strong(),
                                &post.url
                            )
                        );
                        
                        // Post metadata
                        ui.label(
                            egui::RichText::new(format!("Posted by u/{} in r/{}", post.author, post.subreddit))
                                .size(12.0)
                                .weak()
                        );
                        
                        ui.label(
                            egui::RichText::new(format!("Score: {}", post.score))
                                .size(12.0)
                        );
                    });
                });
            });
    }

    fn load_more_posts(&self) {
        if *self.loading.lock().unwrap() {
            return;
        }

        *self.loading.lock().unwrap() = true;
        let after_token = self.after.lock().unwrap().clone();
        let current_subreddit = self.current_subreddit.lock().unwrap().clone();

        let posts = self.posts.clone();
        let loading = self.loading.clone();
        let error_message = self.error_message.clone();
        let reddit_client = self.reddit_client.clone();
        let after = self.after.clone();
        let initial_load = self.initial_load.clone();
        let settings = self.settings.clone();

        thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let client = {
                    let mut client_guard = reddit_client.lock().unwrap();
                    if let Some(client) = client_guard.as_ref() {
                        client.clone()
                    } else {
                        let mut client = match RedditClient::new() {
                            Ok(client) => client,
                            Err(e) => {
                                *error_message.lock().unwrap() = Some(format!("Failed to create client: {}", e));
                                *loading.lock().unwrap() = false;
                                *initial_load.lock().unwrap() = false;
                                return;
                            }
                        };
                        
                        if let Err(e) = client.authenticate(&settings.client_id, &settings.client_secret, 
                            &settings.username, &settings.password).await {
                            *error_message.lock().unwrap() = Some(format!("Authentication error: {}", e));
                            *loading.lock().unwrap() = false;
                            *initial_load.lock().unwrap() = false;
                            return;
                        }
                        
                        *client_guard = Some(client.clone());
                        client
                    }
                };

                let result = if current_subreddit == "home" {
                    client.get_home_feed(after_token.as_deref()).await
                } else {
                    client.get_subreddit_posts(&current_subreddit, after_token.as_deref()).await
                };

                match result {
                    Ok((fetched_posts, new_after)) => {
                        let mut posts_lock = posts.lock().unwrap();
                        if after_token.is_none() && posts_lock.is_empty() {
                            // Only replace posts if we're starting fresh with no posts
                            *posts_lock = fetched_posts;
                        } else {
                            // Otherwise always append
                            posts_lock.extend(fetched_posts);
                        }
                        *after.lock().unwrap() = new_after;
                        *loading.lock().unwrap() = false;
                        *initial_load.lock().unwrap() = false;
                    }
                    Err(e) => {
                        *error_message.lock().unwrap() = Some(format!("Error fetching posts: {}", e));
                        *loading.lock().unwrap() = false;
                        *initial_load.lock().unwrap() = false;
                    }
                }
            });
        });
    }

    fn authenticate_and_load(&self) {
        let settings = self.settings.clone();
        let posts = self.posts.clone();
        let loading = self.loading.clone();
        let error_message = self.error_message.clone();
        let reddit_client = self.reddit_client.clone();
        let initial_load = self.initial_load.clone();
        let subreddits = self.subreddits.clone();
        let loading_subreddits = self.loading_subreddits.clone();

        thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let mut client = match RedditClient::new() {
                    Ok(client) => client,
                    Err(e) => {
                        *error_message.lock().unwrap() = Some(format!("Failed to create client: {}", e));
                        *loading.lock().unwrap() = false;
                        *initial_load.lock().unwrap() = false;
                        return;
                    }
                };
                
                // Authenticate
                if let Err(e) = client.authenticate(&settings.client_id, &settings.client_secret, 
                    &settings.username, &settings.password).await {
                    *error_message.lock().unwrap() = Some(format!("Authentication error: {}", e));
                    *loading.lock().unwrap() = false;
                    *initial_load.lock().unwrap() = false;
                    return;
                }
                
                *reddit_client.lock().unwrap() = Some(client.clone());
                
                // Load subreddits first
                *loading_subreddits.lock().unwrap() = true;
                match client.get_subscribed_subreddits().await {
                    Ok(fetched_subreddits) => {
                        *subreddits.lock().unwrap() = fetched_subreddits;
                        *loading_subreddits.lock().unwrap() = false;
                    }
                    Err(e) => {
                        *error_message.lock().unwrap() = Some(format!("Error fetching subreddits: {}", e));
                        *loading_subreddits.lock().unwrap() = false;
                        *loading.lock().unwrap() = false;
                        *initial_load.lock().unwrap() = false;
                        return;
                    }
                }
                
                // Then fetch posts
                match client.get_home_feed(None).await {
                    Ok((fetched_posts, _after)) => {
                        *posts.lock().unwrap() = fetched_posts;
                        *loading.lock().unwrap() = false;
                        *initial_load.lock().unwrap() = false;
                    }
                    Err(e) => {
                        *error_message.lock().unwrap() = Some(format!("Error fetching posts: {}", e));
                        *loading.lock().unwrap() = false;
                        *initial_load.lock().unwrap() = false;
                    }
                }
            });
        });
    }

    fn refresh_posts(&self) {
        let current = self.current_subreddit.lock().unwrap().clone();
        self.switch_subreddit(current);
    }

    fn handle_scroll_state(&self, ctx: &egui::Context) {
        // Check if the scroll position seems invalid or if we're in an error state
        if let Some(error) = self.error_message.lock().unwrap().as_ref() {
            if error.contains("Error fetching posts") || error.contains("Authentication error") {
                *self.scroll_to_top.lock().unwrap() = true;
            }
        }
        
        // Reset scroll if we have no posts but are not in settings
        if self.posts.lock().unwrap().is_empty() && !self.show_settings {
            *self.scroll_to_top.lock().unwrap() = true;
        }

        // Request a repaint if we're resetting scroll
        if *self.scroll_to_top.lock().unwrap() {
            ctx.request_repaint();
        }
    }

    fn load_subreddits(&self) {
        if *self.loading_subreddits.lock().unwrap() {
            return;
        }

        *self.loading_subreddits.lock().unwrap() = true;
        let reddit_client = self.reddit_client.clone();
        let subreddits = self.subreddits.clone();
        let loading_subreddits = self.loading_subreddits.clone();
        let error_message = self.error_message.clone();
        let settings = self.settings.clone();

        thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let client = {
                    let mut client_guard = reddit_client.lock().unwrap();
                    if let Some(client) = client_guard.as_ref() {
                        client.clone()
                    } else {
                        let mut client = match RedditClient::new() {
                            Ok(client) => client,
                            Err(e) => {
                                *error_message.lock().unwrap() = Some(format!("Failed to create client: {}", e));
                                *loading_subreddits.lock().unwrap() = false;
                                return;
                            }
                        };
                        
                        if let Err(e) = client.authenticate(&settings.client_id, &settings.client_secret, 
                            &settings.username, &settings.password).await {
                            *error_message.lock().unwrap() = Some(format!("Authentication error: {}", e));
                            *loading_subreddits.lock().unwrap() = false;
                            return;
                        }
                        
                        *client_guard = Some(client.clone());
                        client
                    }
                };

                match client.get_subscribed_subreddits().await {
                    Ok(fetched_subreddits) => {
                        *subreddits.lock().unwrap() = fetched_subreddits;
                        *loading_subreddits.lock().unwrap() = false;
                    }
                    Err(e) => {
                        *error_message.lock().unwrap() = Some(format!("Error fetching subreddits: {}", e));
                        *loading_subreddits.lock().unwrap() = false;
                    }
                }
            });
        });
    }

    fn switch_subreddit(&self, subreddit: String) {
        if *self.loading.lock().unwrap() {
            return;
        }

        *self.current_subreddit.lock().unwrap() = subreddit.clone();
        *self.loading.lock().unwrap() = true;
        *self.after.lock().unwrap() = None;  // Reset pagination
        *self.error_message.lock().unwrap() = None;
        *self.initial_load.lock().unwrap() = true;
        *self.scroll_to_top.lock().unwrap() = true;
        
        let reddit_client = self.reddit_client.clone();
        let posts = self.posts.clone();
        let loading = self.loading.clone();
        let error_message = self.error_message.clone();
        let initial_load = self.initial_load.clone();
        let after = self.after.clone();
        let settings = self.settings.clone();

        thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let client = {
                    let mut client_guard = reddit_client.lock().unwrap();
                    if let Some(client) = client_guard.as_ref() {
                        client.clone()
                    } else {
                        let mut client = match RedditClient::new() {
                            Ok(client) => client,
                            Err(e) => {
                                *error_message.lock().unwrap() = Some(format!("Failed to create client: {}", e));
                                *loading.lock().unwrap() = false;
                                *initial_load.lock().unwrap() = false;
                                return;
                            }
                        };
                        
                        if let Err(e) = client.authenticate(&settings.client_id, &settings.client_secret, 
                            &settings.username, &settings.password).await {
                            *error_message.lock().unwrap() = Some(format!("Authentication error: {}", e));
                            *loading.lock().unwrap() = false;
                            *initial_load.lock().unwrap() = false;
                            return;
                        }
                        
                        *client_guard = Some(client.clone());
                        client
                    }
                };

                let result = if subreddit == "home" {
                    client.get_home_feed(None).await
                } else {
                    client.get_subreddit_posts(&subreddit, None).await
                };

                match result {
                    Ok((fetched_posts, new_after)) => {
                        let mut posts_lock = posts.lock().unwrap();
                        *posts_lock = fetched_posts;
                        drop(posts_lock);
                        
                        *after.lock().unwrap() = new_after;
                        *loading.lock().unwrap() = false;
                        *initial_load.lock().unwrap() = false;
                    }
                    Err(e) => {
                        *error_message.lock().unwrap() = Some(format!("Error fetching posts: {}", e));
                        *loading.lock().unwrap() = false;
                        *initial_load.lock().unwrap() = false;
                    }
                }
            });
        });
    }
}

impl eframe::App for RedditApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Set the theme based on settings
        let visuals = if self.settings.dark_mode {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        ctx.set_visuals(visuals);

        // Set longer tooltip delay
        let mut style = (*ctx.style()).clone();
        style.interaction.tooltip_delay = 1.0;
        ctx.set_style(style);

        // Handle scroll state
        self.handle_scroll_state(ctx);

        // Install image loaders (this only needs to happen once)
        static LOADERS_INSTALLED: std::sync::Once = std::sync::Once::new();
        LOADERS_INSTALLED.call_once(|| {
            install_image_loaders(ctx);
        });

        let loading = *self.loading.lock().unwrap();
        if loading {
            ctx.request_repaint();
        }

        // Load subreddits if we haven't yet and we're authenticated
        if self.has_credentials && self.subreddits.lock().unwrap().is_empty() && !*self.loading_subreddits.lock().unwrap() {
            self.load_subreddits();
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading(
                    egui::RichText::new(APP_NAME)
                        .strong()
                        .size(24.0)
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::RIGHT), |ui| {
                    ui.label(egui::RichText::new(APP_VERSION).weak());
                    
                    // Create a container for the refresh button with fixed size
                    ui.allocate_ui_with_layout(
                        egui::vec2(32.0, 32.0),
                        egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                        |ui| {
                            // Only enable refresh button if we have credentials and not showing settings
                            let refresh_button = ui.add_enabled(
                                self.has_credentials && !self.show_settings && !loading,
                                egui::Button::new(
                                    egui::RichText::new("⟳")
                                        .size(16.0)
                                )
                                .min_size(egui::vec2(28.0, 28.0))
                                .rounding(5.0)
                            );
                            if refresh_button.clicked() {
                                self.refresh_posts();
                            }
                        }
                    );
                    
                    // Create a container for the settings button with fixed size
                    ui.allocate_ui_with_layout(
                        egui::vec2(32.0, 32.0),
                        egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                        |ui| {
                            // Only enable settings button if we have credentials
                            let settings_button = ui.add_enabled(
                                self.has_credentials,
                                egui::Button::new(
                                    egui::RichText::new("⚙")
                                        .size(16.0)
                                )
                                .min_size(egui::vec2(28.0, 28.0))
                                .rounding(5.0)
                            );
                            if settings_button.clicked() {
                                self.show_settings = !self.show_settings;
                                if self.show_settings {
                                    *self.error_message.lock().unwrap() = None;
                                }
                            }
                        }
                    );
                });
            });
            ui.add_space(2.0);

            // Subreddit navigation bar
            if self.has_credentials && !self.show_settings {
                ui.horizontal_wrapped(|ui| {
                    let current = self.current_subreddit.lock().unwrap().clone();
                    let subreddits = self.subreddits.lock().unwrap().clone();
                    
                    // Home feed link
                    if ui.add(
                        egui::Button::new(
                            egui::RichText::new("/r/home")
                                .color(if current == "home" {
                                    ui.style().visuals.text_color()
                                } else {
                                    ui.style().visuals.weak_text_color()
                                })
                        ).frame(false)
                    ).clicked() && !loading && current != "home" {
                        self.switch_subreddit("home".to_string());
                    }

                    // Add subreddits with spacing
                    for subreddit in subreddits.iter() {
                        ui.add_space(8.0);
                        if ui.add(
                            egui::Button::new(
                                egui::RichText::new(format!("/r/{}", subreddit))
                                    .color(if current == *subreddit {
                                        ui.style().visuals.text_color()
                                    } else {
                                        ui.style().visuals.weak_text_color()
                                    })
                            ).frame(false)
                        ).clicked() && !loading && current != *subreddit {
                            self.switch_subreddit(subreddit.clone());
                        }
                    }
                });
                ui.separator();
            }
            
            // Error message display (if any)
            if let Some(error) = self.error_message.lock().unwrap().as_ref() {
                ui.add_space(5.0);
                ui.colored_label(ui.style().visuals.error_fg_color, error);
                ui.add_space(5.0);
            }

            // Settings section when visible
            if self.show_settings {
                // Center both horizontally and vertically
                ui.vertical_centered_justified(|ui| {
                    // Add space at the top to help with vertical centering
                    ui.add_space(ui.available_height() * 0.2);
                    
                    let settings_width = 400.0;
                    egui::Frame::group(ui.style())
                        .fill(if self.settings.dark_mode {
                            egui::Color32::from_rgb(20, 20, 20)
                        } else {
                            egui::Color32::from_rgb(240, 240, 240)
                        })
                        .rounding(8.0)  // Add some rounded corners
                        .show(ui, |ui| {
                            ui.set_width(settings_width);
                            ui.vertical_centered(|ui| {
                                ui.add_space(20.0);  // Add some padding at the top
                                if !self.has_credentials {
                                    ui.heading("Welcome to Rustle!");
                                    ui.label("To get started, please enter your Reddit API credentials:");
                                    ui.add_space(10.0);
                                }

                                let label_width = 100.0;
                                let input_width = settings_width - label_width - 40.0;

                                // Add theme toggle at the top
                                ui.horizontal(|ui| {
                                    ui.add_sized([label_width, 20.0], egui::Label::new("Theme:"));
                                    if ui.add_sized([input_width / 2.0, 20.0], 
                                        egui::SelectableLabel::new(!self.settings.dark_mode, "Light")).clicked() {
                                        self.settings.dark_mode = false;
                                        self.settings_modified = true;
                                    }
                                    if ui.add_sized([input_width / 2.0, 20.0], 
                                        egui::SelectableLabel::new(self.settings.dark_mode, "Dark")).clicked() {
                                        self.settings.dark_mode = true;
                                        self.settings_modified = true;
                                    }
                                });
                                ui.add_space(5.0);
                                ui.separator();
                                ui.add_space(5.0);

                                ui.horizontal(|ui| {
                                    ui.add_sized([label_width, 20.0], egui::Label::new("Client ID:"));
                                    if ui.add_sized([input_width, 20.0], egui::TextEdit::singleline(&mut self.settings.client_id)).changed() {
                                        self.settings_modified = true;
                                    }
                                });

                                ui.horizontal(|ui| {
                                    ui.add_sized([label_width, 20.0], egui::Label::new("Client Secret:"));
                                    if ui.add_sized([input_width, 20.0], 
                                        egui::TextEdit::singleline(&mut self.settings.client_secret).password(true)).changed() {
                                        self.settings_modified = true;
                                    }
                                });

                                ui.horizontal(|ui| {
                                    ui.add_sized([label_width, 20.0], egui::Label::new("Username:"));
                                    if ui.add_sized([input_width, 20.0], egui::TextEdit::singleline(&mut self.settings.username)).changed() {
                                        self.settings_modified = true;
                                    }
                                });

                                ui.horizontal(|ui| {
                                    ui.add_sized([label_width, 20.0], egui::Label::new("Password:"));
                                    if ui.add_sized([input_width, 20.0], 
                                        egui::TextEdit::singleline(&mut self.settings.password).password(true)).changed() {
                                        self.settings_modified = true;
                                    }
                                });

                                ui.add_space(10.0);
                                if !self.has_credentials {
                                    ui.label("You can get your Reddit API credentials by:");
                                    ui.label("1. Going to https://www.reddit.com/prefs/apps");
                                    ui.label("2. Scrolling to the bottom and clicking 'create another app...'");
                                    ui.label("3. Selecting 'script' and filling in the required information");
            ui.add_space(10.0);
                                }
                                ui.horizontal(|ui| {
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::RIGHT), |ui| {
                                        if self.has_credentials {
                                            if ui.button("Cancel").clicked() {
                                                self.settings = Settings::load();
                                                self.settings_modified = false;
                                                self.show_settings = false;
                                            }
                                        }
                                        if ui.button("Save").clicked() {
                                            if let Err(e) = self.settings.save() {
                                                *self.error_message.lock().unwrap() = Some(format!("Failed to save settings: {}", e));
                                            } else {
                                                self.settings_modified = false;
                                                self.show_settings = false;
                                                self.has_credentials = true;
                                                *self.error_message.lock().unwrap() = None;
                                                *self.loading.lock().unwrap() = true;
                                                *self.initial_load.lock().unwrap() = true;
                                                *self.scroll_to_top.lock().unwrap() = true;
                                                self.authenticate_and_load();
                                            }
                                        }
                                    });
                                });
                                ui.add_space(20.0);  // Add some padding at the bottom
                            });
                        });
                });
                return;  // Don't show posts while settings are open
            }
            
            // Main content
            let initial_load = *self.initial_load.lock().unwrap();
            
            if initial_load && loading {
                ui.vertical_centered(|ui| {
                    ui.add_space(20.0);
                    ui.spinner();
                    ui.add_space(10.0);
                ui.label("Loading posts...");
                });
            } else {
                let posts = self.posts.lock().unwrap();
                if posts.is_empty() && !self.show_settings {
                    ui.vertical_centered(|ui| {
                        ui.add_space(20.0);
                        ui.label(
                            egui::RichText::new("No posts found.")
                                .size(16.0)
                        );
                    });
                } else {
                    let mut scroll_area = egui::ScrollArea::vertical()
                        .auto_shrink([false; 2]);
                    
                    // Reset scroll position if needed
                    if *self.scroll_to_top.lock().unwrap() {
                        scroll_area = scroll_area.scroll_offset(egui::vec2(0.0, 0.0));
                        *self.scroll_to_top.lock().unwrap() = false;
                    }

                    scroll_area.show(ui, |ui| {
                        for post in posts.iter() {
                            self.render_post(ui, post);
                        }
                        
                        // Check if we're near the bottom and should load more
                        let rect = ui.clip_rect();
                        let max_rect = ui.max_rect();
                        
                        // Get current scroll position from the scroll area
                        let scroll_y = ui.clip_rect().top() - ui.min_rect().top();
                        let mut last_scroll_pos = self.last_scroll_pos.lock().unwrap();
                        
                        // Calculate how far we are from the bottom
                        let distance_from_bottom = max_rect.bottom() - rect.bottom();
                        
                        // Load more posts when we're within 1500px of the bottom
                        // This is much earlier than before to ensure posts are preloaded
                        if !loading && 
                           distance_from_bottom < 1500.0 && 
                           !*self.is_loading_more.lock().unwrap() {
                            
                            // Mark that we're loading more posts
                            *self.is_loading_more.lock().unwrap() = true;
                            
                            // Make sure we have the current after token before loading more
                            let current_after = self.after.lock().unwrap().clone();
                            if current_after.is_none() {
                                // If after is None but we have posts, something is wrong
                                // Reset the after token to ensure we don't replace existing posts
                                if !posts.is_empty() {
                                    *self.after.lock().unwrap() = Some("".to_string());
                                }
                            }
                            
                            self.load_more_posts();
                            
                            // Schedule a delayed reset of the loading more flag
                            let is_loading_more = self.is_loading_more.clone();
                            let repaint_after = std::time::Duration::from_millis(500);
                            std::thread::spawn(move || {
                                std::thread::sleep(repaint_after);
                                *is_loading_more.lock().unwrap() = false;
                            });
                        }
                        
                        // Update the last scroll position
                        *last_scroll_pos = scroll_y;
                        
                        // Show a small loading indicator at the bottom while loading more posts
                        if loading && !initial_load {
                            ui.vertical_centered(|ui| {
                                ui.add_space(10.0);
                                ui.spinner();
                                ui.add_space(10.0);
                            });
                        }
                    });
                }
            }
        });
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, self);
    }

    fn auto_save_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(30)
    }
}

// Add serde support for RedditApp
impl serde::Serialize for RedditApp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("RedditApp", 2)?;
        state.serialize_field("settings", &self.settings)?;
        state.end()
    }
}

impl<'de> serde::Deserialize<'de> for RedditApp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field { Settings }

        struct RedditAppVisitor;

        impl<'de> serde::de::Visitor<'de> for RedditAppVisitor {
            type Value = RedditApp;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct RedditApp")
            }

            fn visit_map<V>(self, mut map: V) -> Result<RedditApp, V::Error>
            where
                V: serde::de::MapAccess<'de>,
            {
                let mut settings = None;
                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Settings => {
                            settings = Some(map.next_value()?);
                        }
                    }
                }

                let settings = settings.unwrap_or_else(Settings::load);
                let has_credentials = !settings.client_id.is_empty() 
                    && !settings.client_secret.is_empty()
                    && !settings.username.is_empty()
                    && !settings.password.is_empty();

                Ok(RedditApp {
                    posts: Arc::new(Mutex::new(Vec::new())),
                    loading: Arc::new(Mutex::new(has_credentials)),
                    error_message: Arc::new(Mutex::new(None)),
                    reddit_client: Arc::new(Mutex::new(None)),
                    after: Arc::new(Mutex::new(None)),
                    initial_load: Arc::new(Mutex::new(has_credentials)),
                    scroll_to_top: Arc::new(Mutex::new(true)), // Always start at top
                    show_settings: !has_credentials,
                    settings,
                    settings_modified: false,
                    has_credentials,
                    current_subreddit: Arc::new(Mutex::new("home".to_string())),
                    subreddits: Arc::new(Mutex::new(Vec::new())),
                    loading_subreddits: Arc::new(Mutex::new(false)),
                    last_scroll_pos: Arc::new(Mutex::new(0.0)),
                    is_loading_more: Arc::new(Mutex::new(false)),
                })
            }
        }

        const FIELDS: &[&str] = &["settings"];
        deserializer.deserialize_struct("RedditApp", FIELDS, RedditAppVisitor)
    }
}

fn main() -> Result<(), eframe::Error> {
    let _args: Vec<String> = std::env::args().collect();
    
    // Load and set the icon
    let icon_data = include_bytes!("../assets/icon.png");
    let (icon_rgba, icon_width, icon_height) = {
        let image = image::load_from_memory(icon_data)
            .expect("Failed to load icon")
            .into_rgba8();
        let (width, height) = image.dimensions();
        let rgba = image.into_raw();
        (rgba, width, height)
    };

    let icon = egui::IconData {
        rgba: icon_rgba,
        width: icon_width,
        height: icon_height,
    };
    
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0])
            .with_min_inner_size([300.0, 200.0])
            .with_title(APP_NAME)
            .with_icon(icon),
        persist_window: true,  // Enable window position/size persistence
        ..Default::default()
    };

    // Create the application state
    let app = RedditApp::new();

    // Only proceed with authentication if we have credentials
    if app.has_credentials {
        let settings = app.settings.clone();
    let posts = app.posts.clone();
    let loading = app.loading.clone();
        let error_message = app.error_message.clone();
        let reddit_client = app.reddit_client.clone();
        let initial_load = app.initial_load.clone();

    // Spawn a thread to handle the async operations
    thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
                let mut client = match RedditClient::new() {
                    Ok(client) => client,
                    Err(e) => {
                        *error_message.lock().unwrap() = Some(format!("Failed to create client: {}", e));
                *loading.lock().unwrap() = false;
                        *initial_load.lock().unwrap() = false;
                return;
            }
                };
                
                // Authenticate
                if let Err(e) = client.authenticate(&settings.client_id, &settings.client_secret, 
                    &settings.username, &settings.password).await {
                    *error_message.lock().unwrap() = Some(format!("Authentication error: {}", e));
                    *loading.lock().unwrap() = false;
                    *initial_load.lock().unwrap() = false;
                    return;
                }
                
                *reddit_client.lock().unwrap() = Some(client);
                
                // Fetch posts
                match reddit_client.lock().unwrap().as_ref().unwrap().get_home_feed(None).await {
                    Ok((fetched_posts, _after)) => {
                        *posts.lock().unwrap() = fetched_posts;
                        *loading.lock().unwrap() = false;
                        *initial_load.lock().unwrap() = false;
                }
                Err(e) => {
                        *error_message.lock().unwrap() = Some(format!("Error fetching posts: {}", e));
                    *loading.lock().unwrap() = false;
                        *initial_load.lock().unwrap() = false;
                    }
                }
            });
        });
    }
    
    // Run the GUI in the main thread
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(|_cc| Box::new(app)),
    )
}
