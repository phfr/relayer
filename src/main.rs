mod chain;
mod imap_client;
pub mod parse_email;
mod processer;
mod smtp_client;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use axum::{
    extract::{Extension, Json, Multipart, Path},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Router,
};
use chain::send_to_chain;
use dotenv::dotenv;
use duct::cmd;
use futures_util::stream::StreamExt;
use imap_client::{EmailReceiver, IMAPAuth};
use mailparse::{self, MailHeaderMap, ParsedMail};
use parse_email::*;
use regex::Regex;
use reqwest;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use smtp_client::EmailSenderClient;
// use sh_caller::run_commands;
use std::fs::File;
use std::io::Read;
use std::{
    collections::hash_map::DefaultHasher,
    env,
    error::Error,
    fs,
    hash::{Hash, Hasher},
    {convert::Infallible, net::SocketAddr},
};
use tracing_subscriber::{
    fmt::{Subscriber, SubscriberBuilder},
    layer::SubscriberExt,
    prelude::*,
    EnvFilter,
};

#[derive(Debug, Deserialize)]
struct EmailEvent {
    dkim: Option<String>,
    subject: Option<String>,
    from: Option<String>,
    to: Option<String>,
}

async fn handle_email(raw_email: String, zk_email_circom_dir: &String) {
    let mut subject = extract_subject(&raw_email).unwrap();
    let mut from = extract_from(&raw_email).unwrap();
    println!("Subject, from: {:?} {:?}", subject, from);

    // Validate subject, and send rejection/reformatting email if necessary
    // let re = Regex::new(r"[Ss]end ?\$?(\d+(\.\d{1,2})?) (eth|usdc) to (.+@.+(\..+)+)").unwrap();
    // if let (Some(to), Some(subject)) = (&email.to, &email.subject) {
    //     let subject_regex = re.clone();
    //     if subject_regex.is_match(subject) {
    //         let custom_reply = format!("{} on Ethereum", subject);
    //         let confirmation = send_custom_reply(to, &custom_reply).await;
    //     }
    // }

    // Path 1: Write raw_email to ../wallet_{hash}.eml
    let hash = {
        let mut hasher = DefaultHasher::new();
        raw_email.hash(&mut hasher);
        hasher.finish()
    };

    let file_path = format!("{}/wallet_{}.eml", "./received_eml", hash);
    match fs::write(file_path.clone(), raw_email.clone()) {
        Ok(_) => println!("Email data written successfully to {}", file_path),
        Err(e) => println!("Error writing data to file: {}", e),
    }
    std::thread::sleep(std::time::Duration::from_secs(3));

    // Path 2: Send to modal
    // let webhook_url = "";
    // let client = reqwest::Client::new();
    // let response = client
    //     .post(webhook_url)
    //     .header("Content-Type", "application/octet-stream")
    //     .body(raw_email)
    //     .send()
    //     .await
    //     .unwrap();

    // Path

    // println!("Response status: {}", response.status());
}

pub async fn validate_email(raw_email: &str, emailer: &EmailSenderClient) {
    let mut subject = extract_subject(&raw_email).unwrap();
    let mut from = extract_from(&raw_email).unwrap();
    println!("Subject, from: {:?} {:?}", subject, from);

    // Validate subject, and send rejection/reformatting email if necessary
    let re = Regex::new(r"[Ss]end ?\$?(\d+(\.\d{1,2})?) (eth|usdc) to (.+@.+(\..+)+)").unwrap();
    let subject_regex = re.clone();
    if subject_regex.is_match(subject.as_str()) {
        let custom_reply = format!("{} on Ethereum", subject);
        let confirmation = emailer.reply_all(raw_email, "Send valid! Validating proof...");
        // .await;
    }
}

const SMTP_DOMAIN_NAME_KEY: &'static str = "SMTP_DOMAIN_NAME";
const SMTP_PORT_KEY: &'static str = "SMTP_PORT";

const IMAP_DOMAIN_NAME_KEY: &'static str = "IMAP_DOMAIN_NAME";
const IMAP_PORT_KEY: &'static str = "IMAP_PORT";
const IMAP_AUTH_TYPE_KEY: &'static str = "AUTH_TYPE";
const IMAP_LOGIN_ID_KEY: &'static str = "IMAP_LOGIN_ID";
const IMAP_LOGIN_PASSWORD_KEY: &'static str = "IMAP_LOGIN_PASSWORD";
const IMAP_CLIENT_ID_KEY: &'static str = "IMAP_CLIENT_ID";
const IMAP_CLIENT_SECRET_KEY: &'static str = "IMAP_CLIENT_SECRET";
const IMAP_AUTH_URL_KEY: &'static str = "IMAP_AUTH_URL";
const IMAP_TOKEN_URL_KEY: &'static str = "IMAP_TOKEN_URL";
const IMAP_REDIRECT_URL_KEY: &'static str = "IMAP_REDIRECT_URL";

const ZK_EMAIL_PATH_KEY: &'static str = "ZK_EMAIL_CIRCOM_PATH";

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    let domain_name = env::var(IMAP_DOMAIN_NAME_KEY)?;
    let zk_email_circom_path = env::var(ZK_EMAIL_PATH_KEY)?;
    let port = env::var(IMAP_PORT_KEY)?.parse()?;
    let auth_type = env::var(IMAP_AUTH_TYPE_KEY)?;
    let imap_auth = if &auth_type == "password" {
        IMAPAuth::Password {
            id: env::var(IMAP_LOGIN_ID_KEY)?,
            password: env::var(IMAP_LOGIN_PASSWORD_KEY)?,
        }
    } else if &auth_type == "oauth" {
        IMAPAuth::OAuth {
            user_id: env::var(IMAP_LOGIN_ID_KEY)?,
            client_id: env::var(IMAP_CLIENT_ID_KEY)?,
            client_secret: env::var(IMAP_CLIENT_SECRET_KEY)?,
            auth_url: env::var(IMAP_AUTH_URL_KEY)?,
            token_url: env::var(IMAP_TOKEN_URL_KEY)?,
            redirect_url: env::var(IMAP_REDIRECT_URL_KEY)?,
        }
    } else {
        panic!("Not supported auth type.");
    };

    let mut receiver = EmailReceiver::construct(&domain_name, port, imap_auth).await?;
    let mut sender: EmailSenderClient = EmailSenderClient::new(
        env::var(IMAP_LOGIN_ID_KEY)?.as_str(),
        env::var(IMAP_LOGIN_PASSWORD_KEY)?.as_str(),
        Some(env::var(SMTP_DOMAIN_NAME_KEY)?.as_str()),
    );
    loop {
        receiver.wait_new_email()?;
        println!("new email!");
        let fetches = receiver.retrieve_new_emails()?;
        for fetched in fetches.into_iter() {
            for fetch in fetched.into_iter() {
                if let Some(e) = fetch.envelope() {
                    println!(
                        "from: {}",
                        String::from_utf8(e.from.as_ref().unwrap()[0].name.unwrap().to_vec())
                            .unwrap()
                    );
                    // println!(
                    //     "to: {}",
                    //     String::from_utf8(e.to.as_ref().unwrap()[0].name.unwrap().to_vec())
                    //         .unwrap()
                    // );
                    let subject_str = String::from_utf8(e.subject.unwrap().to_vec()).unwrap();
                    println!("subject: {}", subject_str);
                } else {
                    println!("no envelope");
                    break;
                }
                if let Some(b) = fetch.body() {
                    let body = String::from_utf8(b.to_vec())?;
                    println!("body: {}", body);
                    validate_email(&body.as_str(), &sender).await;
                    handle_email(body, &zk_email_circom_path).await;
                    // let values = parse_external_eml(&body).await.unwrap();
                    // println!("values: {:?}", values);
                    // let from = extract_from(&body).unwrap();
                    // println!("from: {:?}", from);
                    // let subject = extract_subject(&body).unwrap();
                    // println!("subject: {:?}", subject);
                } else {
                    println!("no body");
                    break;
                }
            }
        }
        // tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
    }
}
