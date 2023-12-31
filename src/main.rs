use axum::{
    extract::{DefaultBodyLimit, Path, State},
    response::Html,
    routing::{get, post},
    Form, Router, Json,
};
use github::{BlogPost, Repo};
use pulldown_cmark::{html, Options, Parser};
use reqwest::{Method, StatusCode};
use serde_derive::{Deserialize, Serialize};
use sqlx::PgPool;
use tower_http::{cors::{Any, CorsLayer}, services::{ServeDir, ServeFile}};
use html_to_string_macro::html;

mod github;
mod utils;

/// this flag is to set up debugging instances to allow self-signed certificates.
#[cfg(not(debug_assertions))]
pub(crate) const ACCEPT_INVALID_CERTS: bool = false;
#[cfg(debug_assertions)]
pub(crate) const ACCEPT_INVALID_CERTS: bool = true;

#[shuttle_runtime::main]
pub async fn shuttle_main(
    #[shuttle_shared_db::Postgres(
        local_uri = "postgresql://localhost/adamfortunecom?user=adam&password={secrets.PASSWORD}"
    )]
    pool: PgPool,
    #[shuttle_secrets::Secrets] _secrets: shuttle_secrets::SecretStore,
) -> shuttle_axum::ShuttleAxum {
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("Migrations failed :(");

    let cors = CorsLayer::new()
        // allow `GET` and `POST` when accessing the resource
        .allow_methods([Method::GET])
        // allow requests from any origin
        .allow_origin(Any);

    let state = AppState::new(pool);

    let app = Router::new()
        .nest_service("/", ServeDir::new("./dist").fallback(ServeFile::new("./dist/index.html")))
        .route("/home", get(home))
        .route("/projects_json", get(projects))
        .route("/projects_json/:project", get(project))
        .route("/blog_json", get(blog))
        .route("/blog_json/:blog", get(blog_post))
        .route("/formatjson", post(format_json))
        .with_state(state.clone())
        .layer(cors)
        .layer(DefaultBodyLimit::max(20_000_000_000)); // raise the limit to 20 GB

    Ok(app.into())
}

async fn home(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    match github::get_home(state.clone()).await {
        None => Err(StatusCode::INTERNAL_SERVER_ERROR), 
        Some(data) => Ok(Html(parse_md_to_html(&data.content))),
    }
}

#[derive(Serialize,Deserialize)]
pub(crate) struct RepoDTO {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) url: String,
    pub(crate) html_url: String,
    pub(crate) description: String,
    pub(crate) readme: Option<String>,
    pub(crate) url_safe_name: String,
    pub(crate) additional_nav_elements: Vec<NavBarElement>,
}

impl From<Repo> for RepoDTO {
    fn from(value: Repo) -> Self {
        Self {
            additional_nav_elements: vec![
                Some(NavBarElement {
                    display_text: "Source Code".to_string(),
                    href: value.html_url.to_string(),
                }),
                match value.name.as_str() {
                    "tree-iterators-rs" => Some(NavBarElement {
                        display_text: "Crates.io".to_string(),
                        href: "https://crates.io/crates/tree_iterators_rs".to_string(),
                    }),
                    "json-formatter" => Some(NavBarElement {
                        display_text: "Crates.io".to_string(),
                        href: "https://crates.io/crates/toy-json-formatter".to_string(),
                    }),
                    _ => None,
                }
            ].into_iter()
            .flat_map(|vec| vec)
            .collect(),
            url_safe_name: get_url_safe_name(&value.name),
            id: value.id,
            name: value.name,
            url: value.url,
            html_url: value.html_url,
            description: value.description,
            readme: if let Some(readme) = value.readme { 
                Some(parse_md_to_html(&readme)) 
            } else { 
                None 
            },
        }
    }
}

async fn projects(State(state): State<AppState>) -> Result<Json<Vec<RepoDTO>>, StatusCode> {
    println!("In projects");
    match github::get_repos(state.clone()).await {
        None => Err(StatusCode::NOT_FOUND),
        Some(data) => {
            Ok(Json(data.into_iter().map(|repo| repo.into()).collect()))
        }
    }
}

async fn project(
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Json<RepoDTO>, StatusCode> {
    match github::get_repo(&state.clone(), &project).await {
        None => Err(StatusCode::NOT_FOUND),
        Some(mut repo) => {
            if let "json-formatter" = repo.name.as_str() {
                if let Some(readme) = &mut repo.readme {
                    *readme = readme.replace(
                        "!Json Formatter Input Box Goes Here!", 
                        &html!{<form action="/formatjson" method="post">
                            <label for="type">"JSON Type:"</label><br/>
                            <input type="radio" id="jsonStandard" name="format" value="JsonStandard" checked />
                            <label for="jsonStandard">"Standard JSON"</label><br/>
                            <input type="radio" id="jsonLines" name="format" value="JsonLines" />
                            <label for="jsonLines">"Json Lines Format"</label><br/>  
                            <label for="json">"JSON:"</label><br/>
                            <textarea id="json" name="json" style="width:100%;min-height:200px;"></textarea><br/>
                            <input type="submit" value="Submit" />
                        </form>})
                }
            }

            Ok(Json(repo.into()))
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct BlogPostDTO {
    pub(crate) id: i32,
    pub(crate) name: String,
    pub(crate) alphanumeric_name: String,
    pub(crate) sha: String,
    pub(crate) description: String,
    pub(crate) content: String,
    pub(crate) url_safe_name: String,
}

impl From<BlogPost> for BlogPostDTO {
    fn from(value: BlogPost) -> Self {
        BlogPostDTO {
            url_safe_name: get_url_safe_name(&value.name),
            id: value.id,
            name: value.name,
            alphanumeric_name: value.alphanumeric_name,
            sha: value.sha,
            description: value.description,
            content: parse_md_to_html(&value.content),
        }
    }
}

async fn blog(State(state): State<AppState>) -> Result<Json<Vec<BlogPostDTO>>, StatusCode> {
    println!("In blog");
    match github::get_blog_posts(state.clone()).await {
        None => Err(StatusCode::NOT_FOUND),
        Some(mut data) => {
            data.sort_by(|post1, post2| post2.description.cmp(&post1.description));
            Ok(Json(data.into_iter().map(|post| post.into()).collect()))
        }
    }
}

async fn blog_post(
    State(state): State<AppState>,
    Path(blog): Path<String>,
) -> Result<Json<BlogPostDTO>, StatusCode> {
    println!("{}", format!("In blog post {blog}"));
    match github::get_blog_post(&state.clone(), &blog).await {
        None => Err(StatusCode::NOT_FOUND),
        Some(blog_post) => Ok(Json(blog_post.into()))
    }
}

async fn format_json(json: Form<JsonFormData>) -> Html<String> {
    let mut result = String::new();

    let jsons;
    match json.0.format {
        JsonFormat::JsonLines => {
            jsons = json.0.json.lines().collect();
        }
        JsonFormat::JsonStandard => {
            jsons = vec![&json.0.json[..]];
        }
    }
    for json in jsons {
        result.push_str("<textarea style='height: 50%; width: 100%;'>");
        let (formatted, errs) = toy_json_formatter::format(json);
        result.push_str(&formatted);

        if let Some(errs) = errs {
            result.push('\n');
            result.push_str("Errors:\n");
            for err in errs {
                result.push_str(&format!("{}", err));
                result.push('\n');
            }
        }
        result.push_str("</textarea>");
    }
    Html(result)
}

fn get_url_safe_name(name: &str) -> String {
    name.chars()
        .filter(|char| match char {
            'a'..='z' | 'A'..='Z' | '0'..='9' => true,
            _ => false,
        })
        .collect()
}

fn parse_md_to_html(md: &str) -> String {
    let parser = Parser::new_ext(&md, Options::empty());
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    html_output
}

#[derive(Serialize, Deserialize)]
struct NavBarElement {
    display_text: String,
    href: String,
}

#[derive(Clone)]
struct AppState {
    db_connection: PgPool,
}

impl AppState {
    fn new(pool: PgPool) -> Self {
        Self {
            db_connection: pool,
        }
    }
}

#[derive(Deserialize)]
struct JsonFormData {
    format: JsonFormat,
    json: String,
}

#[derive(Deserialize)]
enum JsonFormat {
    JsonStandard,
    JsonLines,
}
