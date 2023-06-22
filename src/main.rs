use std::{
    fs::{create_dir_all, read_to_string, File},
    io::Write,
    path::Path,
    process::exit,
    thread::sleep,
    time::Duration,
};

use chrono::prelude::*;
use graphql_client::{reqwest::post_graphql_blocking, GraphQLQuery};
use log::{debug, error};
use preset::LocationPreset;
use reqwest::{
    blocking::Client,
    header::{HeaderMap, HeaderValue, InvalidHeaderValue, AUTHORIZATION},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

mod preset;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let _ = dotenvy::dotenv();
    env_logger::init();

    // Remove blacklisted users
    let blacklist = read_blacklist();

    let token = std::env::var("GITHUB_TOKEN").unwrap();

    let mut headers = HeaderMap::with_capacity(1);
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", token)).expect("Failed to create token header"),
    );

    let client = match Client::builder()
        .user_agent(format!("Committer/{}", VERSION))
        .default_headers(headers)
        .build()
    {
        Ok(value) => value,
        Err(error) => {
            error!("Failed to create request client: {}", error);
            exit(1);
        }
    };

    for preset in preset::PRESETS {
        debug!("Starting preset: {}", preset.title);
        let (users, min_followers) = match search_users(&client, &blacklist, preset) {
            Ok(value) => value,
            Err(err) => {
                error!("Failed to complete preset {}: {}", preset.title, err);
                exit(1);
            }
        };
        if let Err(err) = produce_output(users, preset.title, min_followers) {
            error!(
                "Failed to produce preset output for {}: {}",
                preset.title, err
            );
            exit(1);
        }
        debug!("Finished preset: {}", preset.title);
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Output {
    title: String,
    min_followers: i64,
    generated_at: DateTime<Utc>,
    users: Vec<User>,
}

#[derive(Debug, Error)]
pub enum OutputResult {
    #[error("Error while serializing results: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("Failed to create/write output file: {0}")]
    WriteFile(#[from] std::io::Error),
}

/// Writes the output file for the provided users, title
/// and min followers
///
/// # Arguments
/// * users - The collection of users
/// * title - The name of the preset file
/// * min_followers - The min follower count
fn produce_output(
    mut users: Vec<User>,
    title: &str,
    min_followers: i64,
) -> Result<(), OutputResult> {
    let data = Path::new("data");
    if !data.exists() {
        create_dir_all(data).expect("Failed to create data directory");
    }

    // Sort the results by number of commits
    users.sort_by(|a, b| b.commits.cmp(&a.commits));

    let file_name = title.to_lowercase().replace(' ', "+");
    let out = data.join(format!("{}.json", file_name));

    let output = Output {
        title: title.to_string(),
        min_followers,
        generated_at: Utc::now(),
        users,
    };

    let json: String = serde_json::to_string(&output)?;
    let mut file = File::create(out)?;
    file.write_all(json.as_bytes())?;
    Ok(())
}

/// Reads the collection of blacklisted named from the
/// blacklist.txt file
fn read_blacklist() -> Vec<Box<str>> {
    let path = Path::new("blacklist.txt");
    if !path.exists() {
        return Vec::with_capacity(0);
    }

    let file = read_to_string(path).expect("Failed to read blacklist file");
    file.lines()
        .filter(|line| line.is_empty() || line.starts_with('#'))
        .map(|line| Box::from(line.trim()))
        .collect()
}

#[allow(clippy::upper_case_acronyms)]
type URI = String;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "src/schema.graphql",
    query_path = "src/users.graphql",
    response_derives = "Debug"
)]
struct UsersQuery;

/// Errors that could occur while searching for users
#[derive(Debug, Error)]
enum SearchError {
    #[error("Invalid token header: {0}")]
    InvalidTokenHeader(#[from] InvalidHeaderValue),
    #[error("Ran out of attempts and failed request")]
    FailedRequest(#[from] reqwest::Error),
    #[error("Request encountered errors")]
    RequestErrors,
    #[error("Request missing data")]
    MissingData,
}

#[derive(Debug, Serialize, Deserialize)]
struct User {
    login: String,
    avatar: String,
    name: Option<String>,
    company: Option<String>,
    orgs: Vec<String>,
    followers: i64,
    contribs: i64,
    pub_contribs: i64,
    priv_contribs: i64,
    commits: i64,
    pull_requests: i64,
}

/// Searches for and collects users from GitHub
///
/// # Arguments
/// * client - The client to make the graphql requests
/// * blacklist - List of blacklisted names
/// * location - The location data for the request
fn search_users(
    client: &Client,
    blacklist: &[Box<str>],
    location: &LocationPreset,
) -> Result<(Vec<User>, i64), SearchError> {
    /// GitHub API URL for GraphQL
    const GRAPHQL_URL: &str = "https://api.github.com/graphql";

    /// The number of users to collect (Considered amount)
    const USERS: usize = 1000;

    const PER_PAGE: usize = 5;
    const MAX_PER_QUERY: usize = 1000;

    /// Maximum number of times a request can retry before failing
    const MAX_ATTEMPTS: usize = 10;

    let mut users: Vec<User> = Vec::new();
    let mut last_cursor: Option<String> = None;

    let mut attempts = 0;

    let mut min_followers = -1;

    'outer: while users.len() < USERS {
        let mut query = String::new();

        for location in location.include {
            query.push_str(" location:");
            query.push_str(location);
        }

        for location in location.exclude {
            query.push_str(" -location:");
            query.push_str(location);
        }

        if min_followers >= 0 {
            query.push_str(" followers:<");
            query.push_str(&min_followers.to_string());
        }

        query.push_str(" sort:followers-desc");

        for _ in 1..(MAX_PER_QUERY / PER_PAGE) {
            let variables = users_query::Variables {
                query: query.clone(),
                first: PER_PAGE as i64,
                after: last_cursor.take(),
            };

            let res = match post_graphql_blocking::<UsersQuery, _>(client, GRAPHQL_URL, variables) {
                Ok(value) => value,
                Err(err) => {
                    attempts += 1;
                    if attempts < MAX_ATTEMPTS {
                        error!("Failed request (retry in 10s): {}", err);

                        // Sleep for 10 seconds before trying again
                        sleep(Duration::from_secs(10));
                        continue;
                    } else {
                        return Err(SearchError::FailedRequest(err));
                    }
                }
            };

            if let Some(errors) = res.errors {
                attempts += 1;
                if attempts < MAX_ATTEMPTS {
                    error!("Request errored (retry in 10s): {:?}", errors);

                    // Sleep for 10 seconds before trying again
                    sleep(Duration::from_secs(10));
                    continue;
                } else {
                    return Err(SearchError::RequestErrors);
                }
            }

            let data = match res.data {
                Some(value) => value,
                None => {
                    attempts += 1;
                    if attempts < MAX_ATTEMPTS {
                        error!("Request missing data (retry in 10s)");

                        // Sleep for 10 seconds before trying again
                        sleep(Duration::from_secs(10));
                        continue;
                    } else {
                        return Err(SearchError::MissingData);
                    }
                }
            };

            let edges = match data.search.edges {
                Some(ref value) if value.is_empty() => break 'outer,
                Some(value) => value,
                None => break 'outer,
            };

            edges
                .into_iter()
                .flatten()
                .filter_map(|user| match user.node {
                    Some(users_query::UsersQuerySearchEdgesNode::User(value)) => {
                        Some((user.cursor, value))
                    }
                    _ => None,
                })
                // Skip blacklisted users
                .skip_while(|(_, user)| {
                    blacklist
                        .iter()
                        .any(|blacklist| user.login.eq(blacklist.as_ref()))
                })
                .for_each(|(cursor, user)| {
                    let contrib_count = user
                        .contributions_collection
                        .contribution_calendar
                        .total_contributions;
                    let priv_contrib_count =
                        user.contributions_collection.restricted_contributions_count;
                    let pub_contrib_count = contrib_count - priv_contrib_count;

                    let orgs = if let Some(orgs) = user.organizations.nodes {
                        orgs.into_iter()
                            .flatten()
                            .map(|value| value.login)
                            .collect()
                    } else {
                        Vec::with_capacity(0)
                    };

                    let user = User {
                        login: user.login,
                        avatar: user.avatar_url,
                        name: user.name,
                        company: user.company,
                        orgs,
                        followers: user.followers.total_count,
                        contribs: contrib_count,
                        pub_contribs: pub_contrib_count,
                        priv_contribs: priv_contrib_count,
                        commits: user.contributions_collection.total_commit_contributions,
                        pull_requests: user
                            .contributions_collection
                            .total_pull_request_contributions,
                    };

                    min_followers = user.followers;

                    users.push(user);
                    last_cursor = Some(cursor);
                });

            debug!("Progress: {}/{}", users.len(), USERS);

            if users.len() >= USERS {
                users.truncate(USERS);
                break 'outer;
            }
        }
    }

    Ok((users, min_followers))
}
