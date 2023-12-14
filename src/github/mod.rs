use std::{cmp::Ordering, sync::Arc, time::Duration};

use base64::Engine;
use reqwest::{Client, ClientBuilder};
use serde_derive::{Deserialize, Serialize};
use sqlx::{
    types::chrono::{DateTime, Utc},
    FromRow,
};

use futures::future;

use crate::{get_url_safe_name, AppState};

const URL: &'static str = "https://api.github.com/";
const USERNAME: &'static str = "mr-adult";

pub(crate) async fn get_home(state: AppState) -> Option<BlogPost> {
    tokio::spawn(update_data_if_necessary(state.clone()));

    let result = sqlx::query_as::<_, BlogPost>(
        "SELECT * FROM BlogPosts WHERE alphanumeric_name='Home' LIMIT 1;",
    )
    .fetch_one(&state.db_connection)
    .await
    .ok()?;

    Some(result)
}

pub(crate) async fn get_repos(state: AppState) -> Option<Vec<Repo>> {
    tokio::spawn(update_data_if_necessary(state.clone()));
    get_repos_from_db(state).await
}

async fn get_repos_from_db(state: AppState) -> Option<Vec<Repo>> {
    let result =
        sqlx::query_as::<_, Repo>("SELECT * FROM MrAdultRepositories ORDER BY alphanumeric_name;")
            .fetch_all(&state.db_connection)
            .await
            .ok()?;

    Some(result)
}

pub(crate) async fn get_repo(state: &AppState, name: &str) -> Option<Repo> {
    tokio::spawn(update_data_if_necessary(state.clone()));

    let result = sqlx::query_as::<_, Repo>(
        "SELECT * FROM MrAdultRepositories WHERE alphanumeric_name=$1 LIMIT 1;",
    )
    .bind(get_url_safe_name(&name))
    .fetch_one(&state.db_connection)
    .await
    .ok()?;

    Some(result)
}

pub(crate) async fn get_blog_posts(state: AppState) -> Option<Vec<BlogPost>> {
    tokio::spawn(update_data_if_necessary(state.clone()));

    let result = sqlx::query_as::<_, BlogPost>(
        "SELECT * FROM BlogPosts WHERE alphanumeric_name <> 'Home' ORDER BY alphanumeric_name;",
    )
    .fetch_all(&state.db_connection)
    .await
    .ok()?;

    Some(result)
}

pub(crate) async fn get_blog_post(state: &AppState, name: &str) -> Option<BlogPost> {
    tokio::spawn(update_data_if_necessary(state.clone()));

    let result = sqlx::query_as::<_, BlogPost>(
        "SELECT * FROM BlogPosts WHERE alphanumeric_name=$1 LIMIT 1;",
    )
    .bind(get_url_safe_name(&name))
    .fetch_one(&state.db_connection)
    .await
    .ok()?;

    Some(result)
}

pub(crate) async fn update_data_if_necessary(state: AppState) -> bool {
    if !db_data_is_stale(&state).await {
        return false;
    }

    let timeout = Duration::from_secs(5);
    let client_result = ClientBuilder::new()
        .danger_accept_invalid_certs(crate::ACCEPT_INVALID_CERTS)
        .timeout(timeout)
        .user_agent("adamfortune.com server")
        .build();

    let client = match client_result {
        Ok(inner) => Arc::new(inner),
        Err(_) => return false,
    };

    let mut db_repos = match get_repos_from_db(state.clone()).await {
        None => return false,
        Some(repos) => repos,
    };
    let mut github_repos = match fetch_github_repos(client.clone()).await.ok() {
        None => return false,
        Some(repos) => repos,
    };

    db_repos.sort_by(|repo1, repo2| repo1.id.cmp(&repo2.id));
    github_repos.sort_by(|repo1, repo2| repo1.id.cmp(&repo2.id));

    let max_iterations = github_repos.len() + db_repos.len();

    let mut result = Vec::new();

    let mut db_iter = db_repos.into_iter();
    let mut current_db_value = db_iter.next();
    let mut github_iter = github_repos.into_iter();
    let mut current_github_value = github_iter.next();

    // resolve mismatches
    for _ in 0..max_iterations {
        match &current_github_value {
            None => {
                if let Some(db_val) = current_db_value {
                    println!("Queued repo {} for deletion", db_val.name);
                    result.push((ModificationType::Delete, db_val));
                }
                for item in db_iter {
                    // no corresponding items in github. Delete them!
                    println!("Queued repo {} for deletion", item.name);
                    result.push((ModificationType::Delete, item))
                }
                break;
            }
            Some(github_val) => {
                match &current_db_value {
                    None => {
                        // no corresponding repos in DB. Add them!
                        if let Some(github_val) = current_github_value {
                            println!("Queued repo {} for upsert", github_val.name);
                            result.push((ModificationType::Upsert, github_val));
                        }
                        for item in github_iter {
                            println!("Queued repo {} for upsert", item.name);
                            result.push((ModificationType::Upsert, item));
                        }
                        break;
                    }
                    Some(db_value) => {
                        match github_val.id.cmp(&db_value.id) {
                            Ordering::Less => {
                                println!("Queued repo {} for upsert", github_val.name);
                                result.push((
                                    ModificationType::Upsert,
                                    current_github_value
                                        .expect("github value to be Some() variant"),
                                ));
                                // move the github cursor.
                                current_github_value = github_iter.next();
                            }
                            Ordering::Equal => {
                                if github_val.pushed_at > db_value.pushed_at
                                    || github_val.name == "blog-posts"
                                {
                                    println!("Queued repo {} for upsert", github_val.name);
                                    result.push((
                                        ModificationType::Upsert,
                                        current_github_value
                                            .expect("github value to be Some() variant"),
                                    ));
                                } else {
                                    println!("No changes to {}", github_val.name);
                                    result.push((
                                        ModificationType::None,
                                        current_github_value
                                            .expect("github value to be Some() variant"),
                                    ));
                                }

                                // move both cursors.
                                current_db_value = db_iter.next();
                                current_github_value = github_iter.next();
                            }
                            Ordering::Greater => {
                                println!("Queued repo {} for delete", db_value.name);
                                result.push((
                                    ModificationType::Delete,
                                    current_db_value.expect("db value to be Some() variant"),
                                ));
                                // move the db cursor.
                                current_db_value = db_iter.next();
                            }
                        }
                    }
                }
            }
        };
    }

    let mut repo_modifications = Vec::new();
    let mut repo_deletes = Vec::new();

    for repo in result
        .into_iter()
        .filter(|repo_result| repo_result.0 != ModificationType::None)
    {
        if repo.1.name == "blog-posts" {
            let mut github_blog_posts = match get_all_md_files(&repo.1, &client).await {
                None => return false,
                Some(read_mes) => read_mes,
            };
            let mut db_read_mes = match sqlx::query_as::<_, BlogPost>("SELECT * FROM BlogPosts;")
                .fetch_all(&state.db_connection)
                .await
                .ok()
            {
                None => return false,
                Some(read_mes) => read_mes,
            };

            let max_iterations = github_blog_posts.len() + db_read_mes.len();

            github_blog_posts.sort_by(|readme1, readme2| readme1.name.cmp(&readme2.name));
            db_read_mes.sort_by(|readme1, readme2| readme1.name.cmp(&readme2.name));

            for github_blog_post in github_blog_posts.iter_mut() {
                github_blog_post.name =
                    github_blog_post.name[0..github_blog_post.name.len() - 3].to_string();
                // chop off the ".md"
            }

            let mut read_mes = Vec::with_capacity(github_blog_posts.len());

            let mut db_iter = db_read_mes.into_iter();
            let mut current_db_value = db_iter.next();
            let mut github_iter = github_blog_posts.into_iter();
            let mut current_github_value = github_iter.next();

            // resolve mismatches
            for _ in 0..max_iterations {
                match &current_github_value {
                    None => {
                        if let Some(db_value) = current_db_value {
                            println!("Queued blog post {} for deletion", db_value.name);
                            read_mes.push(BlogModificationType::Delete(db_value));
                        }

                        for item in db_iter {
                            // no corresponding items in github. Delete them!
                            println!("Queued blog post {} for deletion", item.name);
                            read_mes.push(BlogModificationType::Delete(item))
                        }
                        break;
                    }
                    Some(github_val) => {
                        match &current_db_value {
                            None => {
                                if let Some(github_val) = current_github_value {
                                    let path = github_val.path.clone();
                                    read_mes.push(BlogModificationType::Upsert((
                                        github_val,
                                        get_file_content_owned(&repo.1, &client, path),
                                    )));
                                }

                                for item in github_iter {
                                    // no corresponding repo in DB. Add it!
                                    let path = item.path.clone();
                                    println!("Queued blog post {} for upsert", item.name);
                                    read_mes.push(BlogModificationType::Upsert((
                                        item,
                                        get_file_content_owned(&repo.1, &client, path),
                                    )));
                                }
                                break;
                            }
                            Some(db_value) => {
                                match github_val.name.cmp(&db_value.name) {
                                    std::cmp::Ordering::Less => {
                                        let path = github_val.path.clone();
                                        println!("Queued blog post {} for upsert", github_val.name);
                                        read_mes.push(BlogModificationType::Upsert((
                                            current_github_value
                                                .expect("github value to be Some() variants"),
                                            get_file_content_owned(&repo.1, &client, path),
                                        )));
                                        // move the github cursor.
                                        current_github_value = github_iter.next();
                                    }
                                    std::cmp::Ordering::Equal => {
                                        if github_val.sha != db_value.sha {
                                            let path = github_val.path.clone();
                                            println!(
                                                "Queued blog post {} for upsert",
                                                github_val.name
                                            );
                                            read_mes.push(BlogModificationType::Upsert((
                                                current_github_value.expect(
                                                    "current_github_value to be Some() variants",
                                                ),
                                                get_file_content_owned(&repo.1, &client, path),
                                            )));
                                        } else {
                                            println!("No changes to {}", github_val.name);
                                            read_mes.push(BlogModificationType::None);
                                        }

                                        // move both cursors.
                                        current_db_value = db_iter.next();
                                        current_github_value = github_iter.next();
                                    }
                                    std::cmp::Ordering::Greater => {
                                        println!("Queued blog post {} for deletion", db_value.name);
                                        read_mes.push(BlogModificationType::Delete(
                                            current_db_value
                                                .expect("current_db_value to be Some() variant"),
                                        ));
                                        // move the db cursor.
                                        current_db_value = db_iter.next();
                                    }
                                }
                            }
                        };
                    }
                };
            }

            let mut blog_post_upsert_queries = Vec::new();

            for read_me in read_mes.into_iter() {
                match read_me {
                    BlogModificationType::Upsert((metadata, future)) => {
                        // UPSERT
                        let md_content = future.await;

                        let mut description_lines = Vec::new();
                        let mut content_lines = Vec::new();
                        match &md_content {
                            None => {
                                description_lines = Vec::with_capacity(0);
                                content_lines = Vec::with_capacity(0);
                            }
                            Some(content) => {
                                for line in content.lines() {
                                    if line.starts_with("///") {
                                        description_lines.push(line[3..].to_string());
                                    } else {
                                        content_lines.push(line);
                                    }
                                }
                            }
                        }

                        blog_post_upsert_queries.push(
                            sqlx::query(
                                r#"INSERT INTO BlogPosts( name, alphanumeric_name, description, sha, content ) 
                                VALUES ( $1, $2, $3, $4, $5 ) 
                                ON CONFLICT (alphanumeric_name) DO
                                UPDATE SET 
                                    name = EXCLUDED.name,
                                    alphanumeric_name = EXCLUDED.alphanumeric_name,
                                    description = EXCLUDED.description,
                                    sha = EXCLUDED.sha,
                                    content = EXCLUDED.content;"#
                            ).bind(metadata.name.clone())
                                .bind(get_url_safe_name(&metadata.name))
                                .bind(description_lines.join(" "))
                                .bind(metadata.sha)
                                .bind(content_lines.join("\n"))
                                .execute(&state.db_connection)
                        );
                    }
                    BlogModificationType::Delete(blog_post) => {
                        println!("Deleting {}", blog_post.name);
                        blog_post_upsert_queries.push(
                            sqlx::query("DELETE FROM BlogPosts WHERE id=$1;")
                                .bind(blog_post.id)
                                .execute(&state.db_connection),
                        );
                    }
                    BlogModificationType::None => {}
                }
            }

            future::join_all(blog_post_upsert_queries).await;
        } else {
            let db_connection = state.db_connection.clone();
            match repo.0 {
                ModificationType::Delete => repo_deletes.push(async move {
                    match sqlx::query(r#"DELETE FROM MrAdultRepositories WHERE id=$1"#)
                        .bind(repo.1.id)
                        .execute(&db_connection)
                        .await
                    {
                        Ok(_) => {}
                        Err(err) => println!("{}", err),
                    }
                }),
                ModificationType::Upsert => {
                    let mut repo = repo.1;
                    let client = client.clone();
                    let db_connection = state.db_connection.clone();
                    repo_modifications.push(async move {
                        repo.readme = get_read_me(&repo, &client).await;

                        // UPSERT
                        match sqlx::query(
                            r#"INSERT INTO MrAdultRepositories( id, name, alphanumeric_name, url, html_url, description, updated_at, readme ) 
                            VALUES ( $1, $2, $3, $4, $5, $6, $7, $8 ) 
                            ON CONFLICT (id) DO
                            UPDATE SET 
                                name = EXCLUDED.name,
                                alphanumeric_name = EXCLUDED.alphanumeric_name,
                                url = EXCLUDED.url,
                                html_url = EXCLUDED.html_url,
                                description = EXCLUDED.description,
                                updated_at = EXCLUDED.updated_at,
                                readme = EXCLUDED.readme;"#
                        ).bind(repo.id)
                            .bind(repo.name.clone())
                            .bind(get_url_safe_name(&repo.name))
                            .bind(repo.url)
                            .bind(repo.html_url)
                            .bind(repo.description)
                            .bind(repo.pushed_at)
                            .bind(repo.readme)
                            .execute(&db_connection)
                            .await {
                                Ok(_) => {}
                                Err(err) => println!("{}", err)
                            }
                    });
                }
                ModificationType::None => {}
            }
        }
    }

    future::join_all(repo_deletes).await;
    future::join_all(repo_modifications).await;
    return true;
}

async fn db_data_is_stale(state: &AppState) -> bool {
    let time_stamp_result =
        sqlx::query_as::<_, GitHubQueryState>("SELECT * FROM GitHubQueryState LIMIT 1;")
            .fetch_one(&state.db_connection)
            .await
            .ok();

    // failed to get the time stamp for some reason. Treat it as up-to-date.
    if time_stamp_result.is_none() {
        return false;
    }

    let time_stamp: DateTime<Utc>;
    match time_stamp_result {
        // failed to connect. Just treat data as up-to-date
        None => return false,
        Some(time_stamp_result) => {
            time_stamp = time_stamp_result.last_queried;
        }
    }

    if time_stamp < (Utc::now() - chrono::Duration::hours(1)) {
        sqlx::query(
            r#"UPDATE GitHubQueryState SET last_queried = 
                CASE WHEN (last_queried + INTERVAL '1 HOUR') > NOW() 
                THEN last_queried 
                ELSE NOW() 
                END;"#,
        )
        .execute(&state.db_connection)
        .await
        .ok();

        return true;
    } else {
        // up to date - no updates needed.
        return false;
    }
}

async fn fetch_github_repos(client: Arc<Client>) -> Result<Vec<Repo>, ()> {
    let mut get_all_repos_url = URL.to_string();
    get_all_repos_url.push_str(&format!("users/{}/repos", USERNAME));

    let response = client
        .get(&get_all_repos_url)
        .query(&[("username", "mr-adult")])
        .send()
        .await;

    let response = match response {
        Err(_) => Err(())?,
        Ok(inner) => inner,
    };

    let json: Result<Vec<Repo>, _> = response.json().await;

    let repos = match json {
        Err(err) => {
            println!("{:?}", err);
            return Err(());
        }
        Ok(json) => json,
    };

    Ok(repos)
}

async fn get_read_me(repo: &Repo, client: &Client) -> Option<String> {
    get_file_content(repo, client, "README.md").await
}

async fn get_file_content_owned(repo: &Repo, client: &Client, path: String) -> Option<String> {
    get_file_content(repo, client, &path).await
}

async fn get_file_content(repo: &Repo, client: &Client, path: &str) -> Option<String> {
    let mut get_repo_content_url = URL.to_owned();
    get_repo_content_url.push_str(&format!(
        "repos/{}/{}/contents/{}",
        USERNAME, &repo.name, path
    ));

    let response = client
        .get(&get_repo_content_url)
        .query(&[("owner", USERNAME), ("repo", &repo.name), ("path", path)])
        .send()
        .await;

    let response = match response {
        Err(_) => return None,
        Ok(inner) => inner,
    };

    let file_content: Result<Readme, _> = response.json().await;

    match file_content {
        Err(err) => {
            println!("{:?}", err);
            None
        }
        Ok(file_content) => {
            let engine = base64::engine::general_purpose::GeneralPurpose::new(
                &base64::alphabet::STANDARD,
                base64::engine::GeneralPurposeConfig::new(),
            );
            match engine.decode(file_content.content.replace("\n", "")) {
                Err(err) => {
                    println!("{:?}", err);
                    None
                }
                Ok(str) => Some(String::from_utf8_lossy(str.as_slice()).to_string()),
            }
        }
    }
}

async fn get_all_md_files(repo: &Repo, client: &Client) -> Option<Vec<FileMetadata>> {
    let mut get_repo_content_url = URL.to_owned();
    get_repo_content_url.push_str(&format!("repos/{}/{}/contents/", USERNAME, &repo.name));

    let response = client
        .get(&get_repo_content_url)
        .query(&[("owner", USERNAME), ("repo", &repo.name)])
        .send()
        .await;

    let response = match response {
        Err(_) => return None,
        Ok(inner) => inner,
    };

    let files: Result<Vec<FileMetadata>, _> = response.json().await;

    match files {
        Err(err) => {
            crate::utils::log_error(err);
            None
        }
        Ok(files) => {
            return Some(
                files
                    .into_iter()
                    .filter(|file| file.path.ends_with(".md"))
                    .collect(),
            );
        }
    }
}

#[derive(Clone, Debug, Default, FromRow)]
pub struct GitHubQueryState {
    #[allow(unused)]
    id: i32,
    last_queried: DateTime<Utc>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, FromRow)]
pub(crate) struct Repo {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) url: String,
    pub(crate) html_url: String,
    pub(crate) description: String,
    #[sqlx(rename = "updated_at")]
    pub(crate) pushed_at: DateTime<Utc>,
    pub(crate) readme: Option<String>,
}

#[derive(Clone, Default, Deserialize, Serialize, FromRow)]
pub(crate) struct BlogPost {
    pub(crate) id: i32,
    pub(crate) name: String,
    pub(crate) alphanumeric_name: String,
    pub(crate) sha: String,
    pub(crate) description: String,
    pub(crate) content: String,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct FileMetadata {
    sha: String,
    name: String,
    path: String,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct Readme {
    pub(crate) content: String,
}

#[derive(PartialEq, Eq)]
pub(crate) enum ModificationType {
    Delete,
    Upsert,
    None,
}

pub(crate) enum BlogModificationType<T>
where
    T: std::future::Future<Output = Option<String>>,
{
    Delete(BlogPost),
    Upsert((FileMetadata, T)),
    None,
}
