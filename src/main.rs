use futures::TryStreamExt;
use std::collections::HashSet;

use dotenv::dotenv;
use egg_mode::{
    self, auth,
    error::Error,
    tweet::{DraftTweet, Tweet},
    user, Response, Token,
};
use std::env;
use unicode_segmentation::UnicodeSegmentation;

use rand::{prelude::ThreadRng, seq::IteratorRandom, thread_rng};

use lindera::tokenizer::Tokenizer;
use lindera_core::core::viterbi::Mode;

use regex::Regex;

const MIN_WORD_COUNT: usize = 2;
const MIN_ALPHABET_WORD_COUNT: usize = 3;
const SHOULD_FOLLOW_COUNT: usize = 10;
const SHOULD_OTOMAD2_FOLLOW_COUNT: usize = 2;
const FETCH_TWEETS_COUNT: i32 = 100;
const TWEET_SAMPLES_COUNT: usize = 100;
const ALLOWS_RETRY_COUNT: usize = 3;

#[tokio::main]
async fn main() {
    dotenv().ok();

    let url_re = Regex::new(r"https?://(www.)?[-a-zA-Z0-9@:%._+~#=]{1,256}\.[a-zA-Z0-9()]{1,6}\b([-a-zA-Z0-9()@:%_+.~#?&/=]*)").unwrap();
    let space_re = Regex::new(r"[[:space:]]").unwrap();

    let c_key = env::var("CONSUMER_KEY").expect("Please set consumer-key in .env");
    let c_secret = env::var("CONSUMER_SECRET").expect("Please set consumer-secret in .env");
    let a_key = env::var("ACCESS_KEY").expect("Please set access-key in .env");
    let a_secret = env::var("ACCESS_SECRET").expect("Please set access-secret in .env");
    let consumer_token = egg_mode::KeyPair::new(c_key, c_secret);
    let access_token = egg_mode::KeyPair::new(a_key, a_secret);
    let token = egg_mode::Token::Access {
        consumer: consumer_token,
        access: access_token,
    };
    let mut rng = thread_rng();

    // フォローとフォロワーを取得
    let account = auth::verify_tokens(&token).await.expect("Login failed!");
    let friends: HashSet<u64> = user::friends_ids(account.id, &token)
        .map_ok(|r| r.response)
        .try_collect()
        .await
        .unwrap();
    println!("following: {}", friends.len());
    let followers: HashSet<u64> = user::followers_ids(account.id, &token)
        .map_ok(|r| r.response)
        .try_collect()
        .await
        .unwrap();
    println!("followers: {}", followers.len());
    let otomad2_friends: HashSet<u64> = user::friends_ids("otomad2", &token)
        .map_ok(|r| r.response)
        .try_collect()
        .await
        .unwrap();
    println!("otomad2 following: {}", otomad2_friends.len());

    // 差のうちN件をフォロー
    let mut should_follow_ids = followers
        .difference(&friends)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .choose_multiple(&mut rng, SHOULD_FOLLOW_COUNT);
    println!("will follow ids: {:?}", should_follow_ids);

    // otomad2がフォローしていたアカウントをフォロー
    let mut should_follow_ids_2 = otomad2_friends
        .difference(&friends)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .choose_multiple(&mut rng, SHOULD_OTOMAD2_FOLLOW_COUNT);
    println!("and will follow ids: {:?}", should_follow_ids_2);

    should_follow_ids.append(&mut should_follow_ids_2);
    if should_follow_ids.len() == 0 {
        println!("No follow ones");
    } else {
        println!("will follows {} account(s)", should_follow_ids.len());
    }
    for id in should_follow_ids {
        if let Err(err) = user::follow(id, false, &token).await {
            match err {
                egg_mode::error::Error::TwitterError(_, errs) => {
                    let twerr = errs.errors.first().unwrap();
                    if twerr.code == 160 {
                        println!("[SKIP] {}", twerr);
                    }
                }
                _ => {
                    panic!(err)
                }
            }
        };
        println!("followed: {}", id);
    }

    // TLからツイートを取得
    let mut tweet_texts = vec![];
    let home = egg_mode::tweet::home_timeline(&token).with_page_size(FETCH_TWEETS_COUNT);
    let (_home, feed) = home.start().await.unwrap();
    for status in feed.iter() {
        if let Some(retweeted) = status.retweeted {
            if retweeted {
                continue;
            }
        }
        if let Some(lang) = &status.lang {
            match &lang[..] {
                "ja" => {}
                _ => continue,
            }
        }
        // 正規化する
        let mut text = status.text.clone();
        text = url_re.replace_all(&text, "");
        for ht in &status.entities.hashtags {
            let hashed = format!("#{}", ht.text);
            text = text.replace(&hashed, "");
        }
        tweet_texts.push(text);
    }

    // // ツイートをランダムにN件絞る
    let texts = tweet_texts
        .iter()
        .choose_multiple(&mut thread_rng(), TWEET_SAMPLES_COUNT);

    let mut tokenizer = Tokenizer::new(Mode::Normal, "");

    // 形態素解析し、名詞かつ最低文字数以上の単語のみ対象にする
    let mut nouns = vec![];
    for text in texts {
        let tokens = tokenizer.tokenize(text);
        for token in tokens {
            if let Some(detail) = token.detail.get(0) {
                match &detail[..] {
                    "名詞" => {
                        if token.text.graphemes(true).count() >= MIN_WORD_COUNT {
                            nouns.push(token.text);
                        }
                    }
                    "UNK" => {
                        if space_re.replace_all(token.text, "").len() == 0 {
                            continue;
                        }
                        if token.text.graphemes(true).count() >= MIN_ALPHABET_WORD_COUNT {
                            nouns.push(token.text);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    let mut retry_count = 0;
    'outer: loop {
        match create_tweet(&nouns, &mut rng, &token).await {
            Ok(_) => {
                println!("posted");
                break 'outer;
            }
            Err(err) => {
                if retry_count > ALLOWS_RETRY_COUNT {
                    println!("continued {} times but failed {}", retry_count, err);
                    panic!(err);
                }
                println!("continue... {}", retry_count);
                retry_count += 1;
            }
        }
    }

    println!("completed");
}

async fn create_tweet(
    nouns: &Vec<&str>,
    rng: &mut ThreadRng,
    token: &Token,
) -> Result<Response<Tweet>, Error> {
    // ランダムに一語選択する
    let noun = nouns.iter().choose(rng).expect("There is no nouns!");
    println!("音{}", noun);

    // ツイートする
    let tweet = DraftTweet::new(format!("音{}", noun));
    Ok(tweet.send(token).await?)
}
