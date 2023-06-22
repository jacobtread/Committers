use std::{
    fs::{create_dir_all, read_to_string, File},
    io::Write,
    path::Path,
    thread::sleep,
    time::Duration,
};

use graphql_client::{reqwest::post_graphql_blocking, GraphQLQuery};
use log::error;
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

    // Remove blacklisted users
    let blacklist = read_blacklist();

    let token = std::env::var("GITHUB_TOKEN").unwrap();
    let users = search_users(token, &blacklist).unwrap();

    produce_output(users);
}

/// Writes the data/ranked.json file with the sorted results
/// from the users search
///
/// # Arguments
/// * users - The collection of users
fn produce_output(mut users: Vec<User>) {
    let data = Path::new("data");
    if !data.exists() {
        create_dir_all(data).expect("Failed to create data directory");
    }

    // Sort the results by number of commits
    users.sort_by(|a, b| b.commits.cmp(&a.commits));

    let out = data.join("ranked.json");
    let json: String = serde_json::to_string(&users).expect("Failed to create users JSON");
    let mut file = File::create(out).expect("Failed to create data/ranked.json");
    file.write_all(json.as_bytes())
        .expect("Failed to write ranked.json");
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
        .map(Box::from)
        .collect()
}

#[allow(clippy::upper_case_acronyms)]
type URI = String;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "src/schema.docs.graphql",
    query_path = "src/users.graphql",
    response_derives = "Debug"
)]
struct UsersQuery;

/// Errors that could occur while searching for users
#[derive(Debug, Error)]
enum SearchError {
    #[error("Failed to create client: {0}")]
    CreateClient(reqwest::Error),
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
/// * token - The GitHub personal access token
/// * blacklist - List of blacklisted names
fn search_users(token: String, blacklist: &[Box<str>]) -> Result<Vec<User>, SearchError> {
    let locations = &preset::PRESET;

    /// GitHub API URL for GraphQL
    const GRAPHQL_URL: &str = "https://api.github.com/graphql";

    /// The number of users to collect (Considered amount)
    const USERS: usize = 1000;
    /// Number of users to query for each time
    const PER_QUERY: usize = 5;
    /// Maximum number of times a request can retry before failing
    const MAX_ATTEMPTS: usize = 10;

    let mut headers = HeaderMap::with_capacity(1);
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", token))?,
    );

    let client = Client::builder()
        .user_agent(format!("Committer/{}", VERSION))
        .default_headers(headers)
        .build()
        .map_err(SearchError::CreateClient)?;

    let mut users: Vec<User> = Vec::new();
    let mut last_cursor: Option<String> = None;

    let mut attempts = 0;

    while users.len() < USERS {
        let mut query = String::new();

        for location in locations.include {
            query.push_str(" location:");
            query.push_str(location);
        }

        query.push_str(" sort:followers-desc");

        let variables = users_query::Variables {
            query,
            first: PER_QUERY as i64,
            after: last_cursor.take(),
        };

        let res = match post_graphql_blocking::<UsersQuery, _>(&client, GRAPHQL_URL, variables) {
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
            Some(ref value) if value.is_empty() => break,
            Some(value) => value,
            None => break,
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

                users.push(user);
                last_cursor = Some(cursor);
            });
        println!("Progress: {}/{}", users.len(), USERS);
    }

    Ok(users)
}
