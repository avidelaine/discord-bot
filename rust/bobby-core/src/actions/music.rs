//! Port of `bot/actions/music.js`: static texts, action definitions, shortcuts. The `run` functions (the player
//! calls, `item()`) belong to R3b; here only the pure text and shortcut parts.
// Bobby's own Discord music: play a title or a radio style, queue, transport, volume.
// Bobby is the default target of bare "pause", "skip", "volume N", "joue ..."; the PC (actions/pc.rs) needs "sur le PC / Spotify / YouTube Music".
use crate::types::{ActionDef, Echo, Module, QuickFn, Role, SayFn, Step};
use crate::util::{group, re, re_i, replace1};
use fancy_regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;

fn act(kind: &str, text: &str, sub: Option<&str>) -> Vec<Value> {
    match sub {
        Some(k) if !k.is_empty() => vec![json!({ "type": kind, "text": text, "kind": k })],
        _ => vec![json!({ "type": kind, "text": text })],
    }
}

// "musique hardstyle en boucle" -> "hardstyle": the filler words would pollute the YouTube search
static CLEAN_HEAD: LazyLock<Regex> = LazyLock::new(|| re_i(r"^(?:la |de la )?(?:musique|music|chansons?|songs?) (?:de |d'|du |by )?"));
static CLEAN_TAIL: LazyLock<Regex> = LazyLock::new(|| re_i(r" (?:en boucle|en continu|non[- ]?stop|sans arr[êe]t|à l'infini|on repeat|nonstop|songs?|chansons?|musique|music)$"));

fn clean(t: &str) -> String {
    let a = replace1(&CLEAN_HEAD, t, "");
    replace1(&CLEAN_TAIL, &a, "").trim().to_string()
}

const NOTHING: [&str; 2] = ["Rien ne joue.", "Nothing is playing."];

/// JS template rendering of a value (`undefined` for a missing one).
fn js(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "undefined".into(),
        other => other.to_string(),
    }
}

/// JS truthiness.
fn t(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// `a || b`.
fn or(a: &Value, b: &Value) -> String {
    js(if t(a) { a } else { b })
}

fn error_of(s: &Step) -> Option<&str> {
    s.f.get("res").and_then(|r| r["error"].as_str()).filter(|e| !e.is_empty())
}

// A bare "stop" (s.quiet) with nothing playing says nothing at all.
fn quiet(s: &Step) -> bool {
    s.flag("quiet") && error_of(s) == Some("nothing is playing")
}

// Every line goes through here so errors are spoken plainly in both languages.
fn say(s: &Step, lang: &str, f: fn(&Value, &Step, bool) -> String) -> Option<String> {
    let fr = lang != "en";
    if quiet(s) {
        return Some(String::new());
    }
    if let Some(e) = error_of(s) {
        return Some(match e {
            "nothing is playing" => NOTHING[usize::from(!fr)].to_string(),
            "superseded" => String::new(),
            // local mode (Bobby on the PC): the player streams to Discord only
            "no player" => if fr { "La musique ne marche que sur Discord : dis « Bobby, connecte-toi sur Discord ».".into() } else { r#"Music only works on Discord: say "Bobby, join Discord"."#.into() },
            e => if fr { format!("Ça n'a pas marché : {e}.") } else { format!("That didn't work: {e}.") },
        });
    }
    let empty = json!({});
    Some(f(s.f.get("res").unwrap_or(&empty), s, fr))
}

fn is_radio(r: &Value, s: &Step) -> bool {
    r["kind"] == "radio" || s.str("kind") == Some("radio")
}

fn playing_line(r: &Value, s: &Step, fr: bool) -> String {
    if is_radio(r, s) {
        return if fr { format!("Radio {} : {}.", s.text, or(&r["station"], &r["title"])) } else { format!("{} radio: {}.", s.text, or(&r["station"], &r["title"])) };
    }
    if r["endless"] == "artist" {
        return if fr { format!("Que du {} : je lance {}.", js(&r["seed"]), js(&r["title"])) } else { format!("Only {}: playing {}.", js(&r["seed"]), js(&r["title"])) };
    }
    let similar = r["endless"] == "similar";
    if fr {
        format!("Je lance {}{}.", js(&r["title"]), if similar { ", puis la suite dans le même style" } else { "" })
    } else {
        format!("Playing {}{}.", js(&r["title"]), if similar { ", then more in the same style" } else { "" })
    }
}

fn endless_line(e: &Value, fr: bool) -> String {
    let artist = e["mode"] == "artist";
    match (fr, artist) {
        (true, true) => format!("que du {} en continu", js(&e["seed"])),
        (true, false) => format!("en continu dans le style de {}", js(&e["seed"])),
        (false, true) => format!("only {}, nonstop", js(&e["seed"])),
        (false, false) => format!("nonstop in the style of {}", js(&e["seed"])),
    }
}

// echo: done silently after the ack (see brain.js report); errors are always spoken.
fn ok(s: &Step) -> bool {
    error_of(s).is_none()
}

fn s_pause(s: &Step, _: Option<&str>, l: &str) -> Option<String> {
    say(s, l, |_, _, fr| if fr { "Pause.".into() } else { "Paused.".into() })
}
fn s_resume(s: &Step, _: Option<&str>, l: &str) -> Option<String> {
    say(s, l, |_, _, fr| if fr { "Je reprends.".into() } else { "Resuming.".into() })
}
fn s_stop(s: &Step, _: Option<&str>, l: &str) -> Option<String> {
    say(s, l, |_, _, fr| if fr { "Musique arrêtée.".into() } else { "Stopped.".into() })
}

fn prime(echo: Echo, say: SayFn) -> ActionDef {
    ActionDef { role: Role::Prime, echo, say: Some(say), ..Default::default() }
}

fn q(r: Regex, f: QuickFn) -> (Regex, QuickFn) {
    (r, f)
}

static UP: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:(?:monte|augmente) (?:le )?(?:son|volume)|monte|plus fort|louder|turn (?:it )?up|(?:raise|increase|turn up) (?:the )?volume)$"));
static DOWN: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:(?:baisse|diminue|descends) (?:le )?(?:son|volume)|baisse|moins fort|quieter|softer|turn (?:it )?down|(?:lower|decrease|reduce|turn down) (?:the )?volume)$"));
static VOICE_UP: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:parle plus (?:fort|haut)|monte ta voix|augmente ta voix|augmente le volume de ta voix|speak up|speak louder|talk louder|raise your voice)$"));
static VOICE_DOWN: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:parle moins fort|parle plus bas|plus bas|baisse ta voix|baisse le volume de ta voix|speak (?:more )?(?:quietly|softly|quieter|softer)|talk quieter|lower your voice)$"));
static VOICE_SET: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:(?:parle|speak|talk) (?:à|a|at|to) |mets ta voix (?:à|a|sur) |set your voice to )(\d+)(?: ?%| pour cent| percent)?$"));
static SET: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:(?:mets|met|règle|regle) (?:le )?(?:son|volume) (?:à|a|sur) |volume (?:à |a |to )?|set (?:the )?volume to )(\d+)(?: ?%| pour cent| percent)?$"));
static RADIO: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:joue|lance|mets|play|put on) (?:une? |a )?radio (?:de |d'|of )?(.+?)$|^(?:joue|lance|mets|play|put on) (?:some |a )?(.+?) radio$"));
static STYLE: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:joue|lance|mets|play|put on)(?:-moi| me)? (?:du |de la |de l'|des |d'|some |a bit of )(?!(?:la )?(?:musique|music)$)(.+?)$"));
static ONLY: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:joue|lance|mets|play|put on)(?:-moi| me)? (?:que |seulement |uniquement |only )(?:du |de la |de l'|des |d')?(?:musique de |music by |songs by )?(.+?)$"));
static JUST: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:joue|lance|mets|play|put on)(?:-moi| me)? (?:juste |just )(?:le titre |la chanson |le morceau |the song )?(.+?)$"));

pub fn module() -> Module {
    Module {
        name: "music",
        help: [
            "jouer de la musique en continu (un style, un titre, ou que un artiste), une radio, pause, suivant, file d'attente, volume",
            "play music nonstop (a style, a song, or only one artist), a radio, pause, skip, queue, volume",
        ],
        short: ["la musique", "music"],
        prompt: vec![
            r##"- music_play : jouer de la musique avec Bobby, en continu : "text" = un titre ou un artiste ("kind" = youtube), un style ou un genre ("kind" = style), ou une radio quand le mot radio est dit ("kind" = radio). "mode" : similaire par défaut (des morceaux du même style s'enchaînent sans fin, pas forcément du même artiste), artiste quand on ne veut que cet artiste ("que du…", "seulement…", "uniquement…", "only…"), unique pour ce titre seul ("juste ce titre", "rien après")"##,
            r##"- music_add : ajouter à la file d'attente, "text" et "kind" comme music_play"##,
            r##"- music_pause / music_resume / music_stop / music_skip / music_replay : pause, reprendre, arrêter (vide la file), passer au suivant, rejouer"##,
            r##"- music_queue : dire la file d'attente"##,
            r##"- music_volume : volume de Bobby, de 0 à 100 (100 = le maximum) ; "text" vaut up, down ou le nombre demandé (même au-dessus de 100 : Bobby met le maximum et le dit), ou -N%, +N% ou /N pour un changement relatif"##,
            r##"- music_now : dire quel morceau joue"##,
        ],
        rules: vec![
            r##"- Pause, reprise, suivant, volume et "c'est quoi ce son" sans mention du PC, de Spotify ou de YouTube Music concernent la musique de Bobby (music_*), pas pc_media. "kind" = radio seulement si le mot radio est dit, style pour un style ou un genre, sinon youtube. Un seul music_play par demande de musique : la suite dans le même style est automatique."##,
        ],
        fields: vec![("kind", json!({ "enum": ["youtube", "style", "radio"] })), ("mode", json!({ "enum": ["similaire", "artiste", "unique"] }))],
        examples: vec![
            r##""joue Bohemian Rhapsody" -> {"actions":[{"type":"music_play","text":"Bohemian Rhapsody","kind":"youtube"}],"say":""}"##,
            r##""mets une radio jazz" -> {"actions":[{"type":"music_play","text":"jazz","kind":"radio"}],"say":""}"##,
            r##""play some lofi" -> {"actions":[{"type":"music_play","text":"lofi","kind":"style"}],"say":""}"##,
            r##""joue-moi de la hardstyle en boucle" -> {"actions":[{"type":"music_play","text":"hardstyle","kind":"style"}],"say":""}"##,
            r##""mets que du Adèle Castillon" -> {"actions":[{"type":"music_play","text":"Adèle Castillon","kind":"youtube","mode":"artiste"}],"say":""}"##,
            r##""joue Bohemian Rhapsody et ensuite des trucs dans le genre" -> {"actions":[{"type":"music_play","text":"Bohemian Rhapsody","kind":"youtube"}],"say":""}"##,
            r##""joue juste Bohemian Rhapsody, rien après" -> {"actions":[{"type":"music_play","text":"Bohemian Rhapsody","kind":"youtube","mode":"unique"}],"say":""}"##,
            r##""play Around the World by Daft Punk" -> {"actions":[{"type":"music_play","text":"Around the World Daft Punk","kind":"youtube"}],"say":""}"##,
            r##""passe à la suivante" -> {"actions":[{"type":"music_skip"}],"say":""}"##,
            r##""mets le volume à 40" -> {"actions":[{"type":"music_volume","text":"40"}],"say":""}"##,
            r##""baisse le son de 50 %" -> {"actions":[{"type":"music_volume","text":"-50%"}],"say":""}"##,
            r##""ajoute Blinding Lights à la file" -> {"actions":[{"type":"music_add","text":"Blinding Lights","kind":"youtube"}],"say":""}"##,
            r##""what song is this" -> {"actions":[{"type":"music_now"}],"say":""}"##,
            r##""quelle est ta plage de volume" -> {"analyse":{"genre":"capacite","faisable":true},"actions":[],"say":"Mon volume va de 0 à 100 %."}"##,
        ],
        early: None,
        // ponytail: a handful of phrasings per language; the LLM takes the rest (e.g. "add X to the queue").
        quick: vec![
            q(re(r"^(?:mets? (?:la musique )?en pause|mets? pause|pause(?: la musique| the music| music)?)$"), |_| act("music_pause", "", None)),
            q(re(r"^(?:reprends|relance|remets|continue|resume|unpause)(?: la musique| the music| music)?$"), |_| act("music_resume", "", None)),
            q(re(r"^(?:skip(?: (?:this|the) (?:song|track|music))?|next(?: song| track)?|suivante?|passe|saute|(?:musique|morceau|titre|chanson|piste) suivante?|(?:passe|saute) (?:cette |la )?(?:musique|chanson|piste|titre|morceau)|prochaine (?:musique|chanson))$"), |_| act("music_skip", "", None)),
            q(re(r"^(?:stop|arrête|arrete)$"), |_| vec![json!({ "type": "music_stop", "text": "", "quiet": true })]), // stops the music if any, else silence; never leaves
            q(re(r"^(?:(?:stop|arrête|arrete|coupe)(?: la| le| the)? (?:musique|music|radio))$"), |_| act("music_stop", "", None)),
            q(re(r"^(?:rejoue|replay|recommence)(?: (?:ça|ce son|cette musique|le morceau|this|it|the song|this song))?$|^(?:joue|play) (?:ça|it|this|that)(?: again| encore)$"), |_| act("music_replay", "", None)),
            q(re(r"^(?:deep sleep|dors|va dormir|veille profonde|go to sleep)$"), |_| act("deep_sleep", "", None)),
            q(VOICE_UP.clone(), |_| act("voice_volume", "up", None)),
            q(VOICE_DOWN.clone(), |_| act("voice_volume", "down", None)),
            q(VOICE_SET.clone(), |m| act("voice_volume", group(m, 1), None)),
            q(UP.clone(), |_| act("music_volume", "up", None)),
            q(DOWN.clone(), |_| act("music_volume", "down", None)),
            q(SET.clone(), |m| act("music_volume", group(m, 1), None)),
            q(re(r"^(?:c'est quoi|quelle est|quel est|qu'est-ce qui (?:joue|passe))(?: ce| cette| la| le)? (?:son|musique|morceau|titre|chanson)(?: qui (?:joue|passe))?$|^what(?:'s| is) (?:this|that|the) (?:song|track|music|sound)(?: playing)?$|^what(?:'s| is) playing$|^what (?:song|music|track) is (?:this|playing)$|^qu'est-ce qui (?:joue|passe)$"), |_| act("music_now", "", None)),
            q(re(r"^(?:c'est quoi |quelle est |montre |dis-moi )?(?:la )?file(?: d'attente)?$|^(?:what'?s in |show )?(?:the )?queue$"), |_| act("music_queue", "", None)),
            q(re(r"^(?:joue|lance|mets|play|put on)(?:-moi| me)?(?: de la| some)? (?:musique|music)$"), |_| act("music_play", "pop", Some("style"))), // ponytail: no style given = pop
            q(RADIO.clone(), |m| {
                let g = if group(m, 1).is_empty() { group(m, 2) } else { group(m, 1) };
                act("music_play", &clean(g), Some("radio"))
            }),
            q(ONLY.clone(), |m| vec![json!({ "type": "music_play", "text": clean(group(m, 1)), "kind": "youtube", "mode": "artiste" })]),
            q(JUST.clone(), |m| vec![json!({ "type": "music_play", "text": clean(group(m, 1)), "kind": "youtube", "mode": "unique" })]),
            q(STYLE.clone(), |m| act("music_play", &clean(group(m, 1)), Some("style"))),
            q(re(r"^(?:joue|play) (.+)$"), |m| act("music_play", group(m, 1), Some("youtube"))),
        ],
        prompt_lines: None,
        actions: vec![
            ("music_play", ActionDef { role: Role::Prime, text: true, say: Some(|s, _, l| say(s, l, playing_line)), ..Default::default() }),
            (
                "music_add",
                ActionDef {
                    role: Role::Prime,
                    text: true,
                    say: Some(|s, _, l| {
                        say(s, l, |r, s, fr| {
                            if t(&r["started"]) {
                                playing_line(r, s, fr)
                            } else if fr {
                                format!("J'ajoute {} à la file, position {}.", js(&r["label"]), js(&r["position"]))
                            } else {
                                format!("Added {} to the queue, position {}.", js(&r["label"]), js(&r["position"]))
                            }
                        })
                    }),
                    ..Default::default()
                },
            ),
            ("music_pause", prime(Echo::If(ok), s_pause)),
            ("music_resume", prime(Echo::If(ok), s_resume)),
            // always stop: a track still being looked up (current not set yet) must not start after "stop"
            ("music_stop", prime(Echo::If(ok), s_stop)),
            (
                "music_skip",
                prime(Echo::No, |s, _, l| {
                    say(s, l, |r, _, fr| match (t(&r["nowPlaying"]), fr) {
                        (true, true) => format!("Suivant : {}.", js(&r["nowPlaying"]["title"])),
                        (true, false) => format!("Next: {}.", js(&r["nowPlaying"]["title"])),
                        (false, true) => "Plus rien dans la file.".into(),
                        (false, false) => "Nothing left in the queue.".into(),
                    })
                }),
            ),
            (
                "music_replay",
                prime(Echo::If(ok), |s, _, l| say(s, l, |r, _, fr| if fr { format!("Je relance {}.", js(&r["nowPlaying"]["title"])) } else { format!("Replaying {}.", js(&r["nowPlaying"]["title"])) })),
            ),
            (
                "music_queue",
                prime(Echo::No, |s, _, l| {
                    say(s, l, |r, _, fr| {
                        let n = r["queue"].as_array().map_or(0, Vec::len);
                        let title = js(&r["nowPlaying"]["title"]);
                        let plural = if n > 1 { "s" } else { "" };
                        if n > 0 {
                            let label = js(&r["queue"][0]["label"]);
                            if fr { format!("File : {n} titre{plural}, prochain : {label}.") } else { format!("Queue: {n} track{plural}, next: {label}.") }
                        } else if t(&r["endless"]) {
                            if fr { format!("En cours : {title}, {}.", endless_line(&r["endless"], true)) } else { format!("Playing {title}, {}.", endless_line(&r["endless"], false)) }
                        } else if t(&r["nowPlaying"]) {
                            if fr { format!("File vide, en cours : {title}.") } else { format!("Queue empty, playing: {title}.") }
                        } else {
                            NOTHING[usize::from(!fr)].to_string()
                        }
                    })
                }),
            ),
            (
                "music_volume",
                ActionDef {
                    role: Role::Prime,
                    text: true,
                    // "200 %" sets the maximum and says so (it used to set 100 without a word); a plain change stays silent
                    echo: Echo::If(|s| ok(s) && !s.f.get("res").is_some_and(|r| t(&r["clamped"]))),
                    say: Some(|s, _, l| {
                        say(s, l, |r, _, fr| match (t(&r["clamped"]), fr) {
                            (true, true) => "Le maximum c'est 100 %, je le mets à fond.".into(),
                            (true, false) => "100 % is the maximum, so I'm turning it all the way up.".into(),
                            (false, true) => format!("Volume à {}.", js(&r["volume"])),
                            (false, false) => format!("Volume at {}.", js(&r["volume"])),
                        })
                    }),
                    ..Default::default()
                },
            ),
            (
                "deep_sleep",
                ActionDef { say: Some(|_, _, l| Some(if l == "en" { "Good night.".into() } else { "Bonne nuit.".into() })), ..Default::default() },
            ),
            (
                "voice_volume",
                ActionDef {
                    role: Role::Prime,
                    text: true,
                    echo: Echo::If(ok),
                    say: Some(|s, _, l| {
                        say(s, l, |r, s, fr| match (s.text.as_str(), fr) {
                            ("up", true) => "D'accord, je parle plus fort.".into(),
                            ("up", false) => "Okay, I'll speak louder.".into(),
                            ("down", true) => "Je parle moins fort.".into(),
                            ("down", false) => "I'll speak quieter.".into(),
                            (_, true) => format!("Ma voix est à {} %.", js(&r["level"])),
                            (_, false) => format!("My voice is at {} %.", js(&r["level"])),
                        })
                    }),
                    ..Default::default()
                },
            ),
            (
                "music_now",
                prime(Echo::No, |s, _, l| {
                    say(s, l, |r, _, fr| {
                        let np = &r["nowPlaying"];
                        let title = js(&np["title"]);
                        match (t(np), np["kind"] == "radio", fr) {
                            (true, true, true) => format!("C'est la radio {title}."),
                            (true, true, false) => format!("It's the radio {title}."),
                            (true, false, true) => format!("C'est {title}."),
                            (true, false, false) => format!("It's {title}."),
                            (false, _, _) => NOTHING[usize::from(!fr)].to_string(),
                        }
                    })
                }),
            ),
        ],
    }
}

pub const BUSY: &str = "Pas maintenant, je suis en train de faire autre chose.";

/// R3b: port of the module's `run` functions (one per action type).
pub async fn run<L: bobby_proto::infer::Understand + Sync>(s: &mut Step, env: &super::Env<'_, L>, _lang: &str) -> crate::util::Res<Vec<super::UndoOp>> {
    use bobby_proto::msg::{Cap, Endless, MusicItem, VolumeOp};
    let mode = s.str("mode").unwrap_or("");
    // what to play: a radio, a style (played endlessly from YouTube) or a title (endless "similar" unless "unique" or "artiste")
    let item = || match s.str("kind").unwrap_or("") {
        "radio" => MusicItem::Radio { style: s.text.clone() },
        "style" if mode != "artiste" => MusicItem::Style { style: s.text.clone() },
        _ => match mode {
            "unique" => MusicItem::Youtube { query: s.text.clone(), endless: None, seed: None },
            "artiste" => MusicItem::Youtube { query: s.text.clone(), endless: Some(Endless::Artist), seed: Some(s.text.clone()) },
            _ => MusicItem::Youtube { query: s.text.clone(), endless: Some(Endless::Similar), seed: None },
        },
    };
    if s.kind == "deep_sleep" {
        // this step's own `hear` counts for one; anything more is work Bobby would drop by going to sleep
        if env.io.in_flight() > 1 {
            return Err(crate::util::BrainError::Say(crate::util::Say::new(BUSY, "Not now, I'm busy with something else.")));
        }
        // the music stops first (a failure must not keep Bobby awake), then the app unloads its models
        let _ = env.io.run(Cap::MusicStop).await;
        env.io.send(bobby_proto::msg::BrainMsg::DeepSleep);
        return Ok(vec![]);
    }
    let mut clamped = None;
    let cap = match s.kind.as_str() {
        "music_play" => Cap::MusicPlay { item: item(), add: false },
        "music_add" => Cap::MusicPlay { item: item(), add: true },
        "music_pause" => Cap::MusicPause,
        "music_resume" => Cap::MusicResume,
        "music_stop" => Cap::MusicStop,
        "music_skip" => Cap::MusicSkip,
        "music_replay" => Cap::MusicReplay,
        "music_queue" => Cap::MusicQueue,
        "music_now" => Cap::MusicNow,
        "music_volume" => {
            let lower = s.text.to_lowercase();
            let t = crate::util::replace1(&PERCENT, &lower, "");
            let t = t.trim();
            let rel = REL_VOLUME.captures(lower.trim()).ok().flatten();
            let div = t.strip_prefix('/').filter(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())).and_then(|d| d.parse::<f64>().ok()).filter(|d| *d >= 1.0);
            if rel.is_some() || div.is_some() {
                // relative change: from the player's current volume (a zero step just reads it)
                let cur = env.io.run(Cap::MusicVolume { op: VolumeOp::Delta { delta: 0 } }).await?;
                if !cur["error"].is_null() {
                    s.set("res", cur);
                    return Ok(vec![]);
                }
                let cur = cur["volume"].as_f64().unwrap_or(0.0);
                let (raw, dir) = match (&rel, div) {
                    (Some(r), _) => {
                        let k = r[2].parse::<f64>().unwrap_or(0.0) / 100.0;
                        let dir = if k == 0.0 { 0.0 } else if &r[1] == "-" { -1.0 } else { 1.0 };
                        ((cur * (1.0 + dir * k)).round(), dir)
                    }
                    (_, d) => ((cur / d.unwrap_or(1.0)).round(), if d.unwrap_or(1.0) > 1.0 { -1.0 } else { 0.0 }),
                };
                // a change too small to move the rounded volume ("-10 %" at 4) still moves it by one step
                let n = if raw == cur && dir != 0.0 { cur + dir } else { raw };
                clamped = Some(raw > 100.0);
                Cap::MusicVolume { op: VolumeOp::Set { level: n.clamp(0.0, 100.0) as u8 } }
            } else {
            match t {
                "up" => Cap::MusicVolume { op: VolumeOp::Delta { delta: 10 } },
                "down" => Cap::MusicVolume { op: VolumeOp::Delta { delta: -10 } },
                _ if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) => {
                    // "200 %" sets the maximum and says so
                    let n = t.parse::<u128>().unwrap_or(u128::MAX);
                    clamped = Some(n > 100);
                    Cap::MusicVolume { op: VolumeOp::Set { level: n.min(100) as u8 } }
                }
                _ => {
                    s.set("res", json!({ "error": format!("unknown volume \"{}\"", s.text) }));
                    return Ok(vec![]);
                }
            }
            }
        }
        "voice_volume" => {
            let lower = s.text.to_lowercase();
            let t = crate::util::replace1(&PERCENT, &lower, "");
            let t = t.trim();
            match t {
                "up" => Cap::VoiceVolume { op: VolumeOp::Delta { delta: 10 } },
                "down" => Cap::VoiceVolume { op: VolumeOp::Delta { delta: -10 } },
                _ if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) => Cap::VoiceVolume { op: VolumeOp::Set { level: t.parse::<u128>().unwrap_or(u128::MAX).min(100) as u8 } },
                _ => {
                    s.set("res", json!({ "error": format!("unknown volume \"{}\"", s.text) }));
                    return Ok(vec![]);
                }
            }
        }
        _ => return Ok(vec![]),
    };
    let kind = s.kind.clone();
    // the player's own errors (no player, nothing playing...) come as `ok: true` data `{"error": ".."}`;
    // a failed cap is a real failure ("Ça n'a pas marché." or no rights)
    let mut res = env.io.run(cap).await?;
    if res["error"].is_null() {
        match kind.as_str() {
            // always stop: a track still being looked up must not start after "stop"
            "music_stop" if !(t(&res["stopped"]) || t(&res["cleared"])) => res = json!({ "error": "nothing is playing" }),
            "music_now" if res.get("nowPlaying").is_none() => res = json!({ "nowPlaying": res }),
            "music_volume" => {
                if let (Some(c), Some(o)) = (clamped, res.as_object_mut()) {
                    o.insert("clamped".into(), c.into());
                }
            }
            _ => {}
        }
    }
    s.set("res", res);
    Ok(vec![])
}

static REL_VOLUME: LazyLock<Regex> = LazyLock::new(|| re(r"^([+-])(\d+)\s*(?:%|pour ?cent|percent)$"));
static PERCENT: LazyLock<Regex> = LazyLock::new(|| re(r"\s*(?:%|pour ?cent|percent)$"));

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::pc::testkit::{quick, set_test_apps};

    #[test]
    fn quick_regexes_compile() {
        let m = module();
        assert_eq!(m.quick.len(), 21);
        assert_eq!(m.actions.len(), 12);
        assert_eq!(clean("musique hardstyle en boucle"), "hardstyle");
    }

    #[test]
    fn music_cases() {
        set_test_apps();
        let cases = [
            ("pause", "music_pause", ""), ("mets pause", "music_pause", ""), ("mets la musique en pause", "music_pause", ""),
            ("reprends la musique", "music_resume", ""), ("resume", "music_resume", ""), ("skip", "music_skip", ""), ("suivant", "music_skip", ""),
            ("musique suivante", "music_skip", ""), ("next song", "music_skip", ""), ("stop la musique", "music_stop", ""), ("stop", "music_stop", ""),
            ("rejoue", "music_replay", ""), ("monte le son", "music_volume", "up"), ("baisse le volume", "music_volume", "down"),
            ("volume 50", "music_volume", "50"), ("mets le volume à 40", "music_volume", "40"), ("set the volume to 70", "music_volume", "70"),
            ("c'est quoi ce son", "music_now", ""), ("what's this song", "music_now", ""), ("c'est quoi la file d'attente", "music_queue", ""),
            ("lance de la musique", "music_play", "pop"), ("joue Bohemian Rhapsody", "music_play", "bohemian rhapsody"), ("play Daft Punk", "music_play", "daft punk"),
            ("joue du hardstyle", "music_play", "hardstyle"), ("mets une radio jazz", "music_play", "jazz"), ("play some lofi", "music_play", "lofi"),
        ];
        for (phrase, kind, text) in cases {
            let a = quick(phrase).unwrap_or_default();
            assert!(a.len() == 1 && a[0]["type"] == kind && a[0]["text"] == text, "music shortcut: {phrase} -> {a:?}");
        }
        // 2026-11-16: a style plays endlessly from YouTube; "radio" alone means a web radio; "que du X" = only X; "juste X" = X alone
        let qk = |p: &str| {
            let x = &quick(p).unwrap()[0];
            [x["text"].as_str().unwrap().to_string(), x["kind"].as_str().unwrap().to_string(), x["mode"].as_str().unwrap_or("").to_string()]
        };
        assert_eq!(qk("joue du hardstyle"), ["hardstyle", "style", ""]);
        assert_eq!(qk("mets une radio jazz"), ["jazz", "radio", ""]);
        assert_eq!(qk("play some lofi radio"), ["lofi", "radio", ""]);
        assert_eq!(qk("joue-moi que du adèle castillon"), ["adèle castillon", "youtube", "artiste"]);
        assert_eq!(qk("mets uniquement daft punk"), ["daft punk", "youtube", "artiste"]);
        assert_eq!(qk("joue juste bohemian rhapsody"), ["bohemian rhapsody", "youtube", "unique"]);
        assert_eq!(quick("joue Bohemian Rhapsody").unwrap()[0]["kind"], "youtube");
        for p in ["mets pause sur le PC", "monte le son du PC", "musique suivante sur Spotify", "lance Valorant"] {
            assert!(!quick(p).unwrap_or_default().iter().any(|a| a["type"].as_str().unwrap().starts_with("music_")), "not music: {p}");
        }
        assert_eq!(quick("mets pause sur le PC").unwrap()[0]["type"], "pc_media");
        assert_eq!(quick("stop").unwrap()[0]["quiet"], true);
    }

    #[test]
    fn say_lines() {
        let m = module();
        let get = |k: &str| m.actions.iter().find(|(t, _)| *t == k).unwrap().1.say.unwrap();
        let mut s = Step { text: "bohemian".into(), ..Default::default() };
        s.set("res", json!({ "error": "no player" }));
        assert_eq!(get("music_play")(&s, None, "en").unwrap(), r#"Music only works on Discord: say "Bobby, join Discord"."#);
        s.set("res", json!({ "error": "boom" }));
        assert_eq!(get("music_pause")(&s, None, "fr").unwrap(), "Ça n'a pas marché : boom.");
        s.set("res", json!({ "title": "Queen", "endless": "similar" }));
        assert_eq!(get("music_play")(&s, None, "fr").unwrap(), "Je lance Queen, puis la suite dans le même style.");
        s.set("quiet", true);
        s.set("res", json!({ "error": "nothing is playing" }));
        assert_eq!(get("music_stop")(&s, None, "fr").unwrap(), "");
        s.set("res", json!({ "nowPlaying": { "title": "Jazz FM", "kind": "radio" }, "queue": [] }));
        assert_eq!(get("music_now")(&s, None, "en").unwrap(), "It's the radio Jazz FM.");
    }

    use crate::actions::Env;
    use crate::store::Store;
    use crate::testing::{caps, fake_client, FakeLlm};
    use bobby_proto::msg::{Cap, Endless, MusicItem, VolumeOp};
    use std::sync::{Arc, Mutex};

    /// Port of `say(steps, lang)` of the JS test: resolves, runs and describes one step.
    async fn say_of(raw: Value, io: &crate::io::Io, lang: &str) -> String {
        let (ctx, store, llm) = (crate::testing::ctx(), Store::temp(), FakeLlm::new(vec![]));
        let mut steps = crate::resolve::resolve(&[raw], &ctx, &store).unwrap();
        let env = Env { io, store: &store, ctx: &ctx, llm: &llm, post_to: None };
        let s = &mut steps[0];
        assert!(run(s, &env, lang).await.unwrap().is_empty());
        crate::actions::def(&s.kind).unwrap().say.unwrap()(s, None, lang).unwrap()
    }

    #[test]
    fn voice_volume_shortcuts() {
        let cases = [
            ("parle plus fort", "voice_volume", "up"), ("monte ta voix", "voice_volume", "up"), ("louder, speak up", "", ""), ("speak up", "voice_volume", "up"),
            ("parle moins fort", "voice_volume", "down"), ("plus bas", "voice_volume", "down"), ("baisse ta voix", "voice_volume", "down"),
            ("quieter please", "", ""), ("lower your voice", "voice_volume", "down"), ("parle à 50 %", "voice_volume", "50"),
            ("monte le son", "music_volume", "up"), ("plus fort", "music_volume", "up"), ("moins fort", "music_volume", "down"),
            ("monte le volume du PC", "pc_volume", "up"), ("monte le son du PC", "pc_volume", "up"),
        ];
        for (phrase, kind, text) in cases.into_iter().filter(|c| !c.1.is_empty()) {
            crate::actions::pc::testkit::set_test_apps();
            let a = quick(phrase).unwrap_or_default();
            assert!(a.len() == 1 && a[0]["type"] == kind && a[0]["text"] == text, "{phrase} -> {a:?}");
        }
        assert!(!crate::actions::types().contains(&"voice_volume"));
    }

    #[tokio::test]
    async fn voice_volume_sends_cap() {
        let (io, sent) = fake_client(|cap| match cap {
            Cap::VoiceVolume { op: VolumeOp::Set { level } } => Ok(json!({ "level": level })),
            _ => Ok(json!({ "level": 60 })),
        });
        assert_eq!(say_of(json!({"type":"voice_volume","text":"up"}), &io, "fr").await, "D'accord, je parle plus fort.");
        assert_eq!(caps(&sent).pop().unwrap(), Cap::VoiceVolume { op: VolumeOp::Delta { delta: 10 } });
        assert_eq!(say_of(json!({"type":"voice_volume","text":"down"}), &io, "fr").await, "Je parle moins fort.");
        assert_eq!(caps(&sent).pop().unwrap(), Cap::VoiceVolume { op: VolumeOp::Delta { delta: -10 } });
        assert_eq!(say_of(json!({"type":"voice_volume","text":"50"}), &io, "fr").await, "Ma voix est à 50 %.");
        assert_eq!(caps(&sent).pop().unwrap(), Cap::VoiceVolume { op: VolumeOp::Set { level: 50 } });
    }

    #[tokio::test]
    async fn fake_player() {
        // the JS `fake` player: volume 30, "Bohemian Rhapsody" playing, two tracks queued
        let vol = Arc::new(Mutex::new(30i64));
        let cur = json!({ "title": "Bohemian Rhapsody", "kind": "youtube" });
        let (io, sent) = fake_client(move |cap| match cap {
            Cap::MusicPlay { item: MusicItem::Radio { .. }, add: false } => Ok(json!({ "title": "Jazz FM", "kind": "radio", "station": "Jazz FM" })),
            Cap::MusicPlay { item: MusicItem::Youtube { query, .. }, add: false } => Ok(json!({ "title": query, "kind": "youtube" })),
            Cap::MusicPlay { item: MusicItem::Youtube { query, .. }, add: true } => Ok(json!({ "started": false, "position": 3, "label": query })),
            Cap::MusicPlay { .. } => Ok(json!({ "kind": "youtube" })),
            Cap::MusicPause => Ok(json!({ "paused": true })),
            Cap::MusicResume => Ok(json!({ "error": "nothing is playing" })),
            Cap::MusicStop => Ok(json!({ "stopped": cur, "cleared": 2 })),
            Cap::MusicSkip => Ok(json!({ "skipped": cur, "nowPlaying": { "title": "Blinding Lights" } })),
            Cap::MusicReplay => Ok(json!({ "nowPlaying": cur })),
            Cap::MusicQueue => Ok(json!({ "nowPlaying": cur, "queue": [{ "label": "Blinding Lights" }, { "label": "x" }] })),
            Cap::MusicNow => Ok(json!({ "nowPlaying": cur })),
            Cap::MusicVolume { op } => {
                let mut v = vol.lock().unwrap();
                match op {
                    VolumeOp::Set { level } => *v = i64::from(*level),
                    VolumeOp::Delta { delta } => *v += i64::from(*delta),
                    _ => {}
                }
                Ok(json!({ "volume": *v }))
            }
            _ => Ok(Value::Null),
        });
        let last = || caps(&sent).pop().unwrap();
        let yt = |query: &str, endless, seed: Option<&str>| Cap::MusicPlay { item: MusicItem::Youtube { query: query.into(), endless, seed: seed.map(Into::into) }, add: false };
        assert_eq!(say_of(json!({"type":"music_play","text":"Bohemian Rhapsody","kind":"youtube"}), &io, "fr").await, "Je lance Bohemian Rhapsody.");
        assert_eq!(last(), yt("Bohemian Rhapsody", Some(Endless::Similar), None)); // endless by default
        say_of(json!({"type":"music_play","text":"Bohemian Rhapsody","kind":"youtube","mode":"unique"}), &io, "fr").await;
        assert_eq!(last(), yt("Bohemian Rhapsody", None, None));
        say_of(json!({"type":"music_play","text":"Daft Punk","kind":"youtube","mode":"artiste"}), &io, "fr").await;
        assert_eq!(last(), yt("Daft Punk", Some(Endless::Artist), Some("Daft Punk")));
        say_of(json!({"type":"music_play","text":"hardstyle","kind":"style"}), &io, "fr").await;
        assert_eq!(last(), Cap::MusicPlay { item: MusicItem::Style { style: "hardstyle".into() }, add: false });
        assert_eq!(say_of(json!({"type":"music_play","text":"jazz","kind":"radio"}), &io, "fr").await, "Radio jazz : Jazz FM.");
        assert_eq!(last(), Cap::MusicPlay { item: MusicItem::Radio { style: "jazz".into() }, add: false });
        assert_eq!(say_of(json!({"type":"music_play","text":"jazz","kind":"radio"}), &io, "en").await, "jazz radio: Jazz FM.");
        assert_eq!(say_of(json!({"type":"music_add","text":"Blinding Lights","kind":"youtube"}), &io, "fr").await, "J'ajoute Blinding Lights à la file, position 3.");
        assert!(matches!(last(), Cap::MusicPlay { add: true, .. }));
        assert_eq!(say_of(json!({"type":"music_queue"}), &io, "fr").await, "File : 2 titres, prochain : Blinding Lights.");
        assert_eq!(say_of(json!({"type":"music_queue"}), &io, "en").await, "Queue: 2 tracks, next: Blinding Lights.");
        assert_eq!(say_of(json!({"type":"music_volume","text":"up"}), &io, "fr").await, "Volume à 40.");
        assert_eq!(last(), Cap::MusicVolume { op: VolumeOp::Delta { delta: 10 } });
        assert_eq!(say_of(json!({"type":"music_volume","text":"25"}), &io, "en").await, "Volume at 25.");
        assert_eq!(last(), Cap::MusicVolume { op: VolumeOp::Set { level: 25 } });
        // relative changes start from the current volume (25 here)
        assert_eq!(say_of(json!({"type":"music_volume","text":"-50%"}), &io, "en").await, "Volume at 13.");
        assert_eq!(last(), Cap::MusicVolume { op: VolumeOp::Set { level: 13 } });
        assert_eq!(say_of(json!({"type":"music_volume","text":"+20%"}), &io, "en").await, "Volume at 16.");
        assert_eq!(say_of(json!({"type":"music_volume","text":"/2"}), &io, "en").await, "Volume at 8.");
        assert_eq!(say_of(json!({"type":"music_volume","text":"+2000%"}), &io, "en").await, "100 % is the maximum, so I'm turning it all the way up.");
        assert_eq!(say_of(json!({"type":"music_volume","text":"/0"}), &io, "fr").await, "Ça n'a pas marché : unknown volume \"/0\".");
        // a change too small to move the rounded volume still moves it by one
        say_of(json!({"type":"music_volume","text":"4"}), &io, "en").await;
        assert_eq!(say_of(json!({"type":"music_volume","text":"-10%"}), &io, "en").await, "Volume at 3.");
        say_of(json!({"type":"music_volume","text":"0"}), &io, "en").await;
        assert_eq!(say_of(json!({"type":"music_volume","text":"+20%"}), &io, "en").await, "Volume at 1.");
        assert_eq!(say_of(json!({"type":"music_volume","text":"/2"}), &io, "en").await, "Volume at 0.");
        assert_eq!(say_of(json!({"type":"music_volume","text":"-10%"}), &io, "en").await, "Volume at 0.");

        // "200 %" sets the maximum and says so
        assert_eq!(say_of(json!({"type":"music_volume","text":"200 %"}), &io, "en").await, "100 % is the maximum, so I'm turning it all the way up.");
        assert_eq!(last(), Cap::MusicVolume { op: VolumeOp::Set { level: 100 } });
        assert_eq!(say_of(json!({"type":"music_skip"}), &io, "fr").await, "Suivant : Blinding Lights.");
        assert_eq!(say_of(json!({"type":"music_now"}), &io, "fr").await, "C'est Bohemian Rhapsody.");
        assert_eq!(say_of(json!({"type":"music_resume"}), &io, "fr").await, "Rien ne joue.");
        assert_eq!(say_of(json!({"type":"music_resume"}), &io, "en").await, "Nothing is playing.");
        assert_eq!(say_of(json!({"type":"music_stop"}), &io, "en").await, "Stopped.");
    }

    #[tokio::test]
    async fn no_player_is_a_result_not_an_error() {
        let (io, _) = fake_client(|_| Ok(json!({ "error": "no player" })));
        assert_eq!(say_of(json!({"type":"music_pause"}), &io, "en").await, r#"Music only works on Discord: say "Bobby, join Discord"."#);
        // a stop that stopped nothing is "nothing is playing" (a bare "stop" then says nothing)
        let (io, _) = fake_client(|_| Ok(json!({ "stopped": null, "cleared": 0 })));
        assert_eq!(say_of(json!({"type":"music_stop","quiet":true}), &io, "fr").await, "");
        assert_eq!(say_of(json!({"type":"music_stop"}), &io, "fr").await, "Rien ne joue.");
        assert_eq!(say_of(json!({"type":"music_volume","text":"fort"}), &io, "fr").await, "Ça n'a pas marché : unknown volume \"fort\".");
    }

    #[tokio::test]
    async fn a_failed_cap_is_an_error() {
        let (ctx, store, llm) = (crate::testing::ctx(), Store::temp(), FakeLlm::new(vec![]));
        for (e, rights) in [("boom", false), ("50013 Missing Permissions", true)] {
            let (io, _) = fake_client(move |_| Err(e.into()));
            let env = Env { io: &io, store: &store, ctx: &ctx, llm: &llm, post_to: None };
            let mut s = crate::resolve::resolve(&[json!({"type":"music_pause"})], &ctx, &store).unwrap().remove(0);
            assert!(matches!(run(&mut s, &env, "fr").await, Err(crate::util::BrainError::Failed { no_rights, .. }) if no_rights == rights), "{e}");
        }
    }
}
