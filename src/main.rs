use futures::TryStreamExt;
use std::collections::HashSet;

use dotenv::dotenv;
use egg_mode::{self, auth, tweet::DraftTweet, user};
use std::env;
use unicode_segmentation::UnicodeSegmentation;

use rand::{seq::IteratorRandom, thread_rng};

use lindera::tokenizer::Tokenizer;
use lindera_core::core::viterbi::Mode;

const MIN_WORD_COUNT: usize = 2;
const SHOULD_FOLLOW_COUNT: usize = 10;
const FETCH_TWEETS_COUNT: i32 = 100;
const TWEET_SAMPLES_COUNT: usize = 100;

#[tokio::main]
async fn main() {
    dotenv().ok();

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
    let followers: HashSet<u64> = user::followers_ids(account.id, &token)
        .map_ok(|r| r.response)
        .try_collect()
        .await
        .unwrap();

    // 差のうちN件をフォロー
    let should_follow_ids = followers
        .difference(&friends)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .choose_multiple(&mut rng, SHOULD_FOLLOW_COUNT);
    if should_follow_ids.len() == 0 {
        println!("No follow ones");
    } else {
        println!("will follows {} account(s)", should_follow_ids.len());
    }
    for id in should_follow_ids {
        user::follow(id, false, &token).await.unwrap();
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
        tweet_texts.push(status.text.clone())
    }

    // ツイートをランダムにN件絞る
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
                    _ => {}
                }
            }
        }
    }

    // ランダムに一語選択する
    let noun = nouns.iter().choose(&mut rng).expect("There is no nouns!");
    println!("音{}", noun);

    // ツイートする
    let tweet = DraftTweet::new(format!("音{}", noun));
    tweet.send(&token).await.unwrap();
}
