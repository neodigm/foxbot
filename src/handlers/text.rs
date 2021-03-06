use async_trait::async_trait;
use failure::ResultExt;
use tgbotapi::{requests::*, *};
use tokio01::runtime::current_thread::block_on_all;

use super::Status::*;
use crate::models::{Twitter, TwitterAccount};
use crate::needs_field;
use crate::utils::get_message;

pub struct TextHandler;

#[async_trait]
impl super::Handler for TextHandler {
    fn name(&self) -> &'static str {
        "text"
    }

    async fn handle(
        &self,
        handler: &crate::MessageHandler,
        update: &Update,
        _command: Option<&Command>,
    ) -> Result<super::Status, failure::Error> {
        let message = needs_field!(update, message);
        let text = needs_field!(message, text);

        let now = std::time::Instant::now();

        let from = message.from.as_ref().unwrap();

        if text.trim().parse::<i32>().is_err() {
            tracing::trace!("got text that wasn't oob, ignoring");
            return Ok(Ignored);
        }

        tracing::trace!("checking if message was Twitter code");

        let conn = handler
            .conn
            .check_out()
            .await
            .context("unable to check out database")?;

        let row = match Twitter::get_request(&conn, from.id)
            .await
            .context("unable to query twitter requests")?
        {
            Some(row) => row,
            _ => return Ok(Ignored),
        };

        tracing::trace!("we had waiting Twitter code");

        let request_token = egg_mode::KeyPair::new(row.request_key, row.request_secret);

        let con_token = egg_mode::KeyPair::new(
            handler.config.twitter_consumer_key.clone(),
            handler.config.twitter_consumer_secret.clone(),
        );

        let token = block_on_all(egg_mode::access_token(con_token, &request_token, text))
            .context("unable to get twitter access token")?;

        tracing::trace!("got token");

        let access = match token.0 {
            egg_mode::Token::Access { access, .. } => access,
            _ => unimplemented!(),
        };

        tracing::trace!("got access token");

        Twitter::set_account(
            &conn,
            from.id,
            TwitterAccount {
                consumer_key: access.key.to_string(),
                consumer_secret: access.secret.to_string(),
            },
        )
        .await
        .context("unable to set twitter account data")?;

        let mut args = fluent::FluentArgs::new();
        args.insert("userName", fluent::FluentValue::from(token.2));

        let text = handler
            .get_fluent_bundle(from.language_code.as_deref(), |bundle| {
                get_message(&bundle, "twitter-welcome", Some(args)).unwrap()
            })
            .await;

        let message = SendMessage {
            chat_id: from.id.into(),
            text,
            reply_to_message_id: Some(message.message_id),
            ..Default::default()
        };

        handler
            .make_request(&message)
            .await
            .context("unable to send twitter welcome message")?;

        let point = influxdb::Query::write_query(influxdb::Timestamp::Now, "twitter")
            .add_tag("type", "added")
            .add_field("duration", now.elapsed().as_millis() as i64);

        let _ = handler.influx.query(&point).await;

        Ok(Completed)
    }
}
