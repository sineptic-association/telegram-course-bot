use teloxide::{dispatching::dialogue::GetChatId, types::InputFile};
use tokio::sync::oneshot;

use super::{commands::Command, inline_keyboard::make_keyboard, state::State, *};
use crate::interaction_types::TelegramInteraction;
pub type HandleResult = Result<(), Box<dyn Error + Send + Sync>>;

pub async fn message_handler(bot: Bot, msg: Message, me: Me, events: EventSender) -> HandleResult {
    let Some(chat_id) = msg.chat_id() else {
        log::warn!("Unexpected chat ID");
        return Ok(());
    };
    let Some(user_id) = chat_id.as_user() else {
        bot.send_message(chat_id, "Only users can answer").await?;
        return Ok(());
    };

    let Some(text) = msg.text() else {
        bot.send_message(chat_id, "Message should contain text")
            .await?;
        return Ok(());
    };

    match BotCommands::parse(text, me.username()) {
        Ok(Command::Help) => {
            bot.send_message(msg.chat.id, Command::descriptions().to_string())
                .await?;
        }
        Ok(Command::Start) => {
            events.send(Event::StartInteraction(user_id)).await?;
            // let interactions = vec![
            //     TelegramInteraction::Image("assets/gruvbox-nix.png".into()),
            //     TelegramInteraction::Text("2 * 3 = ".into()),
            //     TelegramInteraction::OneOf(vec![5.to_string(), 6.to_string(), 7.to_string()]),
            //     TelegramInteraction::Text("7 - 5 = ".into()),
            //     TelegramInteraction::UserInput,
            // ];
            // let callback =
            //     async |user_id: UserId, result_receiver: oneshot::Receiver<Vec<String>>| {
            //         let result = result_receiver.await.unwrap();
            //         log::info!("got result for user {user_id}: {:?}", result);

            //         log::info!("try aquire state lock");
            //         let _ = STATE.lock().await;
            //         log::info!("state lock acquired");
            //     };
            // set_task_for_user(bot, user_id, interactions, callback).await?;
        }

        Err(_) => {
            let mut state = STATE.lock().await;
            let state = state.entry(user_id).or_insert(State::default());
            match state {
                State::UserEvent {
                    interactions,
                    current,
                    current_id,
                    current_message,
                    answers,
                    channel: _,
                } => match &interactions[*current] {
                    TelegramInteraction::UserInput => {
                        let user_input = msg.text().unwrap().to_owned();

                        bot.delete_message(msg.chat_id().unwrap(), current_message.unwrap())
                            .await?;

                        answers.push(user_input);
                        *current += 1;
                        *current_id = rand::random();

                        progress_on_user_event(bot, chat_id, state).await?;
                    }
                    _ => {
                        bot.send_message(msg.chat.id, "Unexpected input").await?;
                    }
                },
                State::Idle => {
                    bot.send_message(msg.chat.id, "Command not found!").await?;
                }
            }
        }
    }

    Ok(())
}

pub async fn set_task_for_user(
    bot: Bot,
    user_id: UserId,
    interactions: Vec<TelegramInteraction>,
    channel: oneshot::Sender<Vec<String>>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut state = STATE.lock().await;
    let state = state.entry(user_id).or_insert(State::default());

    *state = State::UserEvent {
        interactions: interactions.clone(),
        current: 0,
        current_id: rand::random(),
        current_message: None,
        answers: Vec::new(),
        channel: Some(channel),
    };

    progress_on_user_event(bot, user_id.into(), state).await?;
    Ok(())
}

pub async fn callback_handler(bot: Bot, q: CallbackQuery) -> HandleResult {
    let Some(chat_id) = q.chat_id() else {
        log::warn!("can't get chat id");
        return Ok(());
    };
    let Some(user_id) = chat_id.as_user() else {
        bot.send_message(chat_id, "Only users can answer").await?;
        return Ok(());
    };

    let _ = bot.answer_callback_query(&q.id).await;

    let mut state = STATE.lock().await;
    let Some(state) = state.get_mut(&user_id) else {
        log::debug!("user {user_id} not in dialogue");
        return Ok(());
    };
    let State::UserEvent {
        interactions,
        current,
        current_id,
        current_message,
        answers,
        channel: _,
    } = state
    else {
        log::debug!("user {user_id} in different state");
        bot.send_message(chat_id, "You can answer only to current question")
            .await?;
        return Ok(());
    };

    let Some(response) = q.data else {
        log::error!("reponse data should be assigned");
        return Ok(());
    };

    let response = response.split_whitespace().collect::<Vec<_>>();

    if response[0] != current_id.to_string() {
        log::debug!("user {user_id} answer to previous question");
        // TODO: maybe delete this message
        bot.send_message(chat_id, "You can answer only to current question")
            .await?;
        return Ok(());
    }

    let TelegramInteraction::OneOf(current_choice) = &interactions[*current] else {
        todo!();
    };

    bot.edit_message_text(
        chat_id,
        current_message.unwrap(),
        format!(
            "You choose: {}",
            current_choice[response[1].parse::<usize>().unwrap()]
        ),
    )
    .await?;

    answers.push(response[1].to_string());
    *current += 1;

    progress_on_user_event(bot, chat_id, state).await?;

    log::trace!("user {user_id} chose: {}", response[1]);

    Ok(())
}

pub async fn progress_on_user_event(
    bot: Bot,
    chat_id: ChatId,
    state: &mut State,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let State::UserEvent {
        interactions,
        current,
        current_id,
        current_message,
        answers,
        channel,
    } = state
    else {
        panic!("Unexpected state");
    };
    loop {
        let len = interactions.len();
        if *current >= len {
            channel.take().unwrap().send(answers.clone()).unwrap();
            *state = State::Idle;
            break;
        }
        match &interactions[*current] {
            TelegramInteraction::OneOf(vec) => {
                *current_id = rand::random();
                let message = bot
                    .send_message(chat_id, "ㅤ")
                    .reply_markup(make_keyboard(vec, *current_id))
                    .await?;

                *current_message = Some(message.id);
                break;
            }
            TelegramInteraction::Text(text) => {
                bot.send_message(chat_id, text).await?;
                *current += 1;
                answers.push(String::new());
            }
            TelegramInteraction::UserInput => {
                let message = bot.send_message(chat_id, "Please enter your input").await?;

                *current_message = Some(message.id);
                *current_id = rand::random();
                break;
            }
            TelegramInteraction::Image(path) => {
                bot.send_photo(chat_id, InputFile::file(path)).await?;
                *current += 1;
                answers.push(String::new());
            }
        }
    }
    Ok(())
}
