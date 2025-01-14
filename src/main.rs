use std::fmt::Display;
use std::time::Duration;

use poise::serenity_prelude::{
    self as serenity, ChannelType, CreateMessage, CreateThread, EditMessage, EditThread,
    GetMessages, MessageFlags,
};
type Context<'a> = poise::Context<'a, Data, Error>;

use poise::{CreateReply, Modal};
use rest::get_persons;
use structs::Person;
type ApplicationContext<'a> = poise::ApplicationContext<'a, Data, Error>;
#[derive(Debug)]
pub struct Data {
    conn: sqlx::SqlitePool,
}
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum TopType {
    #[default]
    Normal,
    Information,
    Sonstiges,
}

impl Display for TopType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TopType::Normal => write!(f, "normal"),
            TopType::Information => write!(f, "information"),
            TopType::Sonstiges => write!(f, "sonstiges"),
        }
    }
}
type Error = Box<dyn std::error::Error + Send + Sync>;

mod database;
mod keycloak;
mod rest;
mod structs;

#[tokio::main]
async fn main() {
    let token = std::env::var("DISCORD_TOKEN").expect("missing DISCORD_TOKEN");
    let intents = serenity::GatewayIntents::non_privileged();

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![antrag(), edit(), abmelden(), information(), sonstiges()],
            ..Default::default()
        })
        .setup(|ctx, _ready, framework| {
            Box::pin(async move {
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                Ok(Data {
                    conn: database::connect().await.expect(""),
                })
            })
        })
        .build();

    let client = serenity::ClientBuilder::new(token, intents)
        .framework(framework)
        .await;
    client.unwrap().start().await.unwrap();
}

#[derive(Debug, Modal, Default)]
#[name = "Antrag Erstellen"]
struct CreateTopModal {
    #[name = "Antragstitel"]
    #[placeholder = ""]
    name: String,
    #[name = "Der FSR Informatik möge beschließen, dass:"]
    #[paragraph]
    #[placeholder = ""]
    antragstext: String,
    #[name = "Begründung"]
    #[paragraph]
    begründung: Option<String>,
}

#[derive(Debug, Modal, Default)]
#[name = "Antrag Editieren"]
struct EditTopModal {
    #[name = "Antragstitel"]
    #[placeholder = ""]
    name: String,
    #[name = "Der FSR Informatik möge beschließen, dass"]
    #[placeholder = ""]
    #[paragraph]
    antragstext: String,
    #[name = "Begründung"]
    #[paragraph]
    begründung: Option<String>,
}

#[poise::command(slash_command)]
pub async fn antrag(ctx: ApplicationContext<'_>) -> Result<(), Error> {
    let top_type = TopType::Normal;
    create_antrag(ctx, top_type).await;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn information(ctx: ApplicationContext<'_>) -> Result<(), Error> {
    let top_type = TopType::Information;
    create_antrag(ctx, top_type).await;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn sonstiges(ctx: ApplicationContext<'_>) -> Result<(), Error> {
    let top_type = TopType::Sonstiges;
    create_antrag(ctx, top_type).await;
    Ok(())
}

pub async fn create_antrag(ctx: ApplicationContext<'_>, top_type: TopType) -> Result<(), Error> {
    let top = CreateTopModal::execute_with_defaults(
        ctx,
        CreateTopModal {
            antragstext: "".to_string(),
            ..Default::default()
        },
    )
    .await?
    .unwrap();

    let name = top.name;
    let antragstext = format!(
        "Der Fachschaftsrat Informatik möge beschließen, dass:\n {}",
        &top.antragstext
    );

    let antragssteller = database::get_name(ctx.data().conn.clone(), ctx.author().id).await;

    let Ok(antragssteller) = antragssteller else {
        ctx.send(
            CreateReply::default()
                .content("Du bist nicht in der Datenbank")
                .ephemeral(true),
        )
        .await
        .unwrap();
        return Ok(());
    };

    let begruendung = &top
        .begründung
        .unwrap_or_else(|| "Keine Begründung".to_string());

    let channel_id = ctx.interaction.channel_id;

    let builder = CreateMessage::new()
        .content(name.clone() + " - " + &antragssteller.name)
        .tts(false);
    let message = channel_id.send_message(&ctx.http(), builder).await;

    let builder = CreateThread::new(&name);
    let thread = channel_id
        .create_thread_from_message(&ctx.http(), message.unwrap().id, builder)
        .await
        .unwrap();

    let builder = CreateMessage::new().content(&antragstext).tts(true);
    thread.clone().id.send_message(&ctx.http(), builder).await?;

    let builder = CreateMessage::new()
        .content(format!("Begründung: \r{}", begruendung))
        .tts(true);
    thread.id.send_message(&ctx.http(), builder).await?;

    let antrag = structs::CreateAntrag {
        titel: name,
        antragstext,
        begründung: begruendung.to_string(),
        antragssteller: vec![antragssteller.id],
    };

    let resp = rest::create_antrag(antrag).await;

    let _ = database::map_antrag_thread(ctx.data().conn.clone(), resp.id, thread.id.into()).await;

    Ok(())
}

#[poise::command(slash_command)]
pub async fn edit(ctx: ApplicationContext<'_>) -> Result<(), Error> {
    let mut channel = ctx.guild_channel().await.unwrap();

    if channel.kind != ChannelType::PublicThread
        && channel.kind != ChannelType::PrivateThread
        && channel.kind != ChannelType::NewsThread
    {
        return Err("This command can only be used in a thread".into());
    }

    //get the messageid of the oldest two messages in the channel
    let gm = GetMessages::new();
    let mut messages = channel.id.messages(&ctx.http(), gm).await?;

    //invert messages
    let mut messages: Vec<_> = messages.drain(..).rev().collect();

    //create modal with the name of the thread
    let modal = EditTopModal::execute_with_defaults(
        ctx,
        EditTopModal {
            name: channel.clone().name,
            antragstext: messages[1].content.to_string(),
            begründung: Some(messages[2].content.replace("Begründung: \r", "")),
        },
    )
    .await?
    .unwrap();

    let threadid = channel.id;
    let parentchannel = channel.parent_id.unwrap();
    let parentmessage = parentchannel.message(&ctx.http(), threadid.get()).await?;
    let split: Vec<&str> = parentmessage.content.split(" - ").collect();
    let name = modal.name;

    let antragssteller = &split[&split.len() - 1].to_owned();

    let antragstext = format!(
        "Der Fachschaftsrat Informatik möge beschließen, dass:\n{}",
        &modal.antragstext
    );

    let begruendung = &modal
        .begründung
        .unwrap_or_else(|| "Keine Begründung".to_string());

    //edit thread title
    let editthread = EditThread::new().name(&name);
    channel.edit_thread(&ctx.http(), editthread).await?;

    //edit the messages
    let builder = EditMessage::new().content(antragstext.to_string());
    messages[1].edit(&ctx.http(), builder).await?;

    let builder = EditMessage::new().content(format!("Begründung: \r{}", begruendung));
    messages[2].edit(&ctx.http(), builder).await?;

    //get the message that startet the thread
    let message = channel.id.message(&ctx.http(), messages[0].id).await?;
    let messagetype = message.kind;

    //if the message is a thread starter message, edit the content

    if messagetype == serenity::model::channel::MessageType::ThreadStarterMessage {
        let threadid = channel.id;
        let parentchannel = channel.parent_id.unwrap();
        let mut parentmessage = parentchannel.message(&ctx.http(), threadid.get()).await?;
        let builder = EditMessage::new().content(name.clone() + " - " + &antragssteller);
        parentmessage.edit(&ctx.http(), builder).await?;
    }

    //TODO: maybe Antragssteller should not be overwritten
    let antrag = structs::EditAntrag {
        id: database::get_antrag_thread(ctx.data().conn.clone(), channel.id.into())
            .await
            .unwrap(),
        titel: name,
        antragstext: antragstext.to_string(),
        begründung: begruendung.to_string(),
        creators: get_persons()
            .await
            .iter()
            .filter(|person| person.name == *antragssteller)
            .map(|person| person.id)
            .collect(),
    };

    rest::edit_antrag(antrag).await;

    Ok(())
}

#[poise::command(slash_command)]
async fn abmelden(ctx: ApplicationContext<'_>) -> Result<(), Error> {
    let person = database::get_name(ctx.data().conn.clone(), ctx.author().id).await;
    let Ok(person) = person else {
        ctx.send(
            CreateReply::default()
                .content("Du bist nicht in der Datenbank")
                .ephemeral(true),
        )
        .await
        .unwrap();
        return Ok(());
    };
    rest::put_abmeldung(person.name.clone()).await;
    let response = format!("{} hat sich abgemeldet!", person.name);
    ctx.say(response).await?;
    Ok(())
}
