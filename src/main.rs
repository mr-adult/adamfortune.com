use axum::{
    extract::{DefaultBodyLimit, Path, State},
    response::Html,
    routing::{get, post},
    Form, Router,
};
use github::{BlogPost, Repo};
use html_to_string_macro::html;
use pulldown_cmark::{html, Options, Parser};
use reqwest::{Method, StatusCode};
use serde_derive::Deserialize;
use sqlx::PgPool;
use tower_http::cors::{Any, CorsLayer};

mod github;
mod utils;

/// this flag is to set up debugging instances to allow self-signed certificates.
#[cfg(not(debug_assertions))]
pub(crate) const ACCEPT_INVALID_CERTS: bool = false;
#[cfg(debug_assertions)]
pub(crate) const ACCEPT_INVALID_CERTS: bool = true;

const ERROR_RESPONSE: &'static str = "Failed to reach database.";

const ALL_PAGES_CSS: &'static str = include_str!("./index.css");

const CONTENT_LIST_CSS: &'static str = include_str!("./content_list.css");

const MARKDOWN_CSS: &'static str = r#"

"#;

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
        .route("/", get(index))
        .route("/projects", get(projects))
        .route("/projects/:project", get(project))
        .route("/blog", get(blog))
        .route("/blog/:blog", get(blog_post))
        .route("/formatjson", post(format_json))
        .with_state(state.clone())
        .layer(cors)
        .layer(DefaultBodyLimit::max(20_000_000_000)); // raise the limit to 20 GB

    Ok(app.into())
}

async fn index(State(state): State<AppState>) -> Html<String> {
    match github::get_home(state.clone()).await {
        None => Html(ERROR_RESPONSE.to_string()),
        Some(data) => {
            return Html(html!(
                {create_html_page(false)}
                <body onLoad="onLoad()">
                    {create_nav_bar(None)}
                    <div style="margin-left: 8px;">
                        {&parse_md_to_html(&data.content)}
                    </div>
                </body>
            ));
        }
    }
}

async fn projects(State(state): State<AppState>) -> Html<String> {
    match github::get_repos(state.clone()).await {
        None => Html(ERROR_RESPONSE.to_string()),
        Some(data) => {
            return Html(html!{
                {create_html_page(true)}
                <body onLoad="onLoad()">
                    {create_nav_bar(None)}
                    <ul style="display: grid; column-count: 1; column-gap: 20px; row-gap: 20px; padding: 0px; word-break: break-word">
                        {data.into_iter()
                            .enumerate()
                            .map(|(i, repo)| generate_repo_card(i, &repo))
                            .collect::<String>()}
                    </ul>
                </body>
            });
        }
    }
}

async fn project(
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Html<String>, StatusCode> {
    match github::get_repo(&state.clone(), &project).await {
        None => Err(StatusCode::NOT_FOUND),
        Some(mut repo) => {
            match repo.name.as_str() {
                "json-formatter" => {
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
                            </form>}
                        );
                    }
                }
                _ => {} // do nothing
            }

            let mut additional_nav_bar_elements = vec![NavBarElement {
                display_text: "Source Code".to_string(),
                href: repo.html_url,
            }];

            match repo.name.as_str() {
                "tree-iterators-rs" => additional_nav_bar_elements.push(NavBarElement {
                    display_text: "Crates.io".to_string(),
                    href: "https://crates.io/crates/tree_iterators_rs".to_string(),
                }),
                "json-formatter" => additional_nav_bar_elements.push(NavBarElement {
                    display_text: "Crates.io".to_string(),
                    href: "https://crates.io/crates/toy-json-formatter".to_string(),
                }),
                _ => {}
            }


            Ok(Html(html!{
                {create_html_page(false)}
                <body onLoad="onLoad()">
                    {create_nav_bar(Some(additional_nav_bar_elements))}
                    {parse_md_to_html(&repo.readme.unwrap_or("".to_string()))}
                </body>
            }))
        }
    }
}

async fn blog(State(state): State<AppState>) -> Html<String> {
    match github::get_blog_posts(state.clone()).await {
        None => Html(ERROR_RESPONSE.to_string()),
        Some(mut data) => {
            data.sort_by(|post1, post2| post2.description.cmp(&post1.description));
            Html(html!{
                {create_html_page(true)}
                <body onLoad="onLoad">
                    {create_nav_bar(None)}
                    <ul style="display: grid; column-count: 2; column-gap: 20px; row-gap: 20px; padding: 0px; word-break: break-word;">
                        {data.into_iter()
                            .enumerate()
                            .map(|(i, blog_post)| generate_blog_card(i, &blog_post))
                            .collect::<String>()}
                    </ul>
                </body>
            })
        }
    }
}

async fn blog_post(
    State(state): State<AppState>,
    Path(blog): Path<String>,
) -> Result<Html<String>, StatusCode> {
    match github::get_blog_post(&state.clone(), &blog).await {
        None => Err(StatusCode::NOT_FOUND),
        Some(blog_post) => {
            Ok(Html(html!{
                {create_html_page(false)}
                <body onLoad="onLoad()">
                    {create_nav_bar(None)}
                    {parse_md_to_html(&blog_post.content)}
                </body>
            }))
        }
    }
}

async fn format_json(json: Form<JsonFormData>) -> Html<String> {
    let mut result = create_html_page(false);
    result.push_str("<body>");

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
        let (formatted, errs) = toy_json_formatter::format(json);
        result.push_str(&html!{
            <textarea style="width: 100%;">
                {if errs.as_ref()
                    .unwrap_or(&Vec::with_capacity(0))
                    .len() > 0 { "Errors:\n" } else { "" }}
                {errs.unwrap_or(Vec::with_capacity(0))
                    .into_iter()
                    .map(|err| format!("{}\n", err))
                    .collect::<String>()}
                {formatted}
            </textarea>
        });
    }
    result.push_str("</body>");
    Html(result)
}

/// Creates an HTML page, adding the <head> tag that is needed.
/// Callers should add the <body> tag and all inner content
fn create_html_page(is_content_list: bool) -> String {
    let mut html = String::from("<!DOCTYPE html>");
    html.push_str("<head>");
    {
        html.push_str(r#"
<link rel="stylesheet" href="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/styles/atom-one-dark.min.css">
<script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/highlight.min.js"></script>

<script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/languages/rust.min.js"></script>
<script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/languages/python.min.js"></script>
<script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/languages/csharp.min.js"></script>
"#);
        html.push_str("<script>");
        {
            html.push_str(include_str!("./onload.js"))
        }
        html.push_str("</script>");
        html.push_str("<script type='module'>");
        {
            html.push_str("import mermaid from 'https://cdn.jsdelivr.net/npm/mermaid@10/dist/mermaid.esm.min.mjs';");
        }
        html.push_str("</script>");

        html.push_str("<style>");
        {
            html.push_str(ALL_PAGES_CSS);
            if is_content_list {
                html.push_str(CONTENT_LIST_CSS);
            } else {
                html.push_str(MARKDOWN_CSS);
            }
        }
        html.push_str("</style>");
    }
    html.push_str("</head>");
    html
}

fn create_nav_bar(additional_elements: Option<Vec<NavBarElement>>) -> String {
    let mut html = String::new();
    html.push_str("<nav id='navbar'>");
    {
        html.push_str("<ul id='navbar_list' style='list-style: none; display: flex; flex-direction: row; justify-content: space-around; margin: 0px; padding: 0px;'>");
        {
            let buttons = [
                NavBarElement {
                    display_text: "Home".to_string(),
                    href: "/".to_string(),
                },
                NavBarElement {
                    display_text: "Projects".to_string(),
                    href: "/projects".to_string(),
                },
                NavBarElement {
                    display_text: "Blog".to_string(),
                    href: "/blog".to_string(),
                },
            ]
            .into_iter()
            .chain(additional_elements.into_iter().flat_map(|opt| opt));

            for element in buttons {
                html.push_str("<li>");
                {
                    html.push_str("<a href='");
                    html.push_str(&element.href);
                    html.push_str("'>");
                    html.push_str(&element.display_text);
                    html.push_str("</a>");
                }
            }
        }
        html.push_str("</ul>");
    }
    html.push_str("</nav>");
    html
}

fn generate_repo_card(index: usize, repo: &Repo) -> String {
    let mut html = String::new();
    html.push_str(&format!(
        "<li style='grid-row: {}; grid-column: {}'>",
        index + 1,
        1
    ));
    {
        html.push_str("<h2>");
        {
            html.push_str(&format!(
                "<a href='/projects/{}'>",
                get_url_safe_name(&repo.name)
            ));
            {
                html.push_str(&repo.name);
            }
            html.push_str("</a>");
        }
        html.push_str("</h2>");

        html.push_str("<p>");
        {
            html.push_str(&repo.description);
        }
        html.push_str("</p>");
    }
    html.push_str("</li>");
    html
}

fn generate_blog_card(index: usize, blog_post: &BlogPost) -> String {
    let mut html = String::new();
    html.push_str(&format!(
        "<li style='grid-row: {}; grid-column: {}'>",
        index + 1,
        1
    ));
    {
        html.push_str("<h2>");
        {
            html.push_str(&format!(
                "<a href='/blog/{}'>",
                get_url_safe_name(&blog_post.name)
            ));
            {
                html.push_str(&blog_post.name);
            }
            html.push_str("</a>");
        }
        html.push_str("</h2>");

        html.push_str("<p>");
        {
            html.push_str(&blog_post.description);
        }
        html.push_str("</p>");
    }
    html.push_str("</li>");
    html
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
