//! Port of unit.test.js batch 2 (journal of 2026-11-12, 22:12-22:24).
//! Batch 2 never goes through hear(): it resolves and runs plans directly, so these tests do the same, with a
//! `Brain` over a fake client (`io.post` -> `Cap::Post`, `io.memberName` -> `Cap::MemberName`, `io.music` ->
//! `Cap::Music*`, the PC dry run -> `Cap::Pc*`).
//! Assertions ported: 78 / 78 JS assert call sites of batch2 (loops count once, as in JS; `assert.rejects` counted).
//! Node-only detail: pc._test.calls[0] starting with "taskkill.exe" is the PC client's business now; the brain
//! test checks the `Cap::PcClose` it sends instead.
use crate::actions::{self, info, Env};
use crate::brain::{lines, Brain};
use crate::guard::{about_bobby, guard, note_guard};
use crate::journal::{iso_now_ms, Memory};
use crate::resolve::{names, needed, resolve};
use crate::roles::role_of;
use crate::shortcuts::{confirms, quick};
use crate::store::{temp_dir, Store};
use crate::testing::{caps, ctx, fake_client, FakeLlm, Sent};
use crate::types::{By, Ctx, Role, Step};
use crate::util::{hit, re, Hist, Res};
use bobby_proto::msg::Cap;
use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const WAKANDEX: &str = "261531632589078528";

/// The JS regex `src` (JS semantics) matches `s`.
fn matches(src: &str, s: &str) -> bool {
    hit(&re(src), s)
}

struct Rig {
    brain: Brain<FakeLlm>,
    sent: Sent,
}

/// `answer` plays the JS fake `io` (post -> 'dev-bot-test', memberName -> the id, music...).
fn rig_with(answer: impl Fn(&Cap) -> Result<Value, String> + Send + 'static) -> Rig {
    let (io, sent) = fake_client(answer);
    let path = temp_dir("batch2").join("conversations.jsonl");
    Rig { brain: Brain::new(Store::temp(), FakeLlm::new(vec![]), io, path), sent }
}

fn rig() -> Rig {
    rig_with(|cap| match cap {
        Cap::Post { .. } => Ok(json!({ "channel": "dev-bot-test" })),
        Cap::MemberName { id } => Ok(json!({ "name": id })),
        Cap::PcClose { name } => Ok(json!({ "found": name })),
        _ => Ok(Value::Null),
    })
}

/// `as(role, name)`.
fn as_(role: Role, name: &str) -> Ctx {
    let mut c = ctx();
    c.role = role;
    c.speaker = c.members.iter().find(|m| m.name == name).unwrap().clone();
    c
}

/// Node `describe(plan)`, in French (Brain::describe is private to brain.rs).
fn describe(plan: &[Step]) -> String {
    plan.iter()
        .filter_map(|s| {
            if s.kind == "undo" {
                return Some(lines("fr").undo(s.flag("done")).to_string());
            }
            let say = actions::def(&s.kind)?.say?;
            let n = s.targets.as_ref().map(|t| names(t, "fr"));
            say(s, n.as_deref(), "fr")
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

impl Rig {
    async fn run(&self, plan: &mut [Step], c: &Ctx) -> Res<()> {
        let env = Env { io: &self.brain.io, store: &self.brain.store, ctx: c, llm: &self.brain.llm, post_to: None };
        self.brain.run_plan(plan, &env, "fr").await
    }

    /// `resolve` then `run` (Node `run(resolve(...), io)`): the error of either, as text.
    async fn try_run(&self, actions: &[Value], c: &Ctx) -> Result<Vec<Step>, String> {
        let mut p = resolve(actions, c, &self.brain.store).map_err(|e| e.fr)?;
        self.run(&mut p, c).await.map_err(|e| e.to_string())?;
        Ok(p)
    }

    /// `go(actions, c)`: resolve, run, describe.
    async fn go(&self, actions: Value, c: &Ctx) -> String {
        let p = self.try_run(actions.as_array().unwrap(), c).await.unwrap();
        describe(&p)
    }

    async fn rejects(&self, actions: Value, c: &Ctx) -> String {
        self.try_run(actions.as_array().unwrap(), c).await.expect_err("should have been rejected")
    }

    fn texts(&self) -> Vec<String> {
        self.brain.store.load_or("todo", json!([])).as_array().unwrap().iter().map(|x| x["text"].as_str().unwrap_or("").to_string()).collect()
    }

    fn todo(&self) -> Vec<Value> {
        self.brain.store.load_or("todo", json!([])).as_array().unwrap().clone()
    }

    fn posts(&self) -> Vec<Cap> {
        caps(&self.sent).into_iter().filter(|c| matches!(c, Cap::Post { .. })).collect()
    }
}

/// `plan(sentence)`: resolve(quick(sentence), ctx), with the plain test context.
fn plan_of(r: &Rig, sentence: &str, c: &Ctx) -> Vec<Step> {
    resolve(&quick(sentence).unwrap(), c, &r.brain.store).unwrap()
}

#[tokio::test]
async fn todo_lists() {
    let r = rig();
    let w = ctx();
    r.brain.store.save("todo", &json!([])).unwrap();
    let (prime, wak, alex) = (as_(Role::Prime, "Prime Ilias"), as_(Role::Super, "WakandeX"), as_(Role::Prime, "Alex"));
    assert_eq!(r.go(json!([{ "type": "todo_add", "text": "Ilias doit avoir absolument tous les droits sur le bot" }]), &prime).await, "C'est noté dans la liste générale.");
    let it = r.todo()[0].clone();
    assert_eq!(
        (it["list"].clone(), it["author"].clone(), it["done"].clone(), it["at"].is_string()),
        (json!("general"), json!({ "id": "397478613559148545", "name": "Prime Ilias" }), json!(false), true)
    );
    assert_eq!(role_of("397478613559148545", &[], &r.brain.store), Role::Admin); // a note never grants anything (admin = .env default here)
    r.go(json!([{ "type": "todo_add", "text": "acheter du pain", "list": "moi" }]), &alex).await;
    assert_eq!(r.todo()[1]["list"], "Alex");
    assert!(matches("liste générale ou à ta liste", &r.rejects(json!([{ "type": "todo_add", "text": "x", "list": "Ilias" }]), &alex).await));
    assert!(matches("tes propres points", &r.rejects(json!([{ "type": "todo_done", "text": "droits" }]), &alex).await)); // Ilias's item
    assert_eq!(r.go(json!([{ "type": "todo_done", "text": "pain" }]), &alex).await, "Fait : acheter du pain");
    assert_eq!(
        r.go(json!([{ "type": "todo_done", "text": "dernier", "list": "general" }]), &wak).await,
        "Fait : Ilias doit avoir absolument tous les droits sur le bot" // super edits everything
    );
    r.go(
        json!([{ "type": "todo_add", "text": "Mettre des fondus dans la musique" }, { "type": "todo_add", "text": "Une voix différente pour Claude" }, { "type": "todo_add", "text": "Nettoyer dev-bot-test" }, { "type": "todo_add", "text": "Quatrième point" }]),
        &wak,
    )
    .await;
    let before = r.posts().len();
    let said = r.go(json!([{ "type": "todo_list" }]), &wak).await;
    assert!(
        matches(r"^La liste générale : 4 points\. Mettre des fondus dans la musique ; Une voix différente pour Claude ; Nettoyer dev-bot-test ; et la suite\. Tout est dans dev-bot-test\.$", &said),
        "{said}"
    );
    let Cap::Post { embeds, .. } = r.posts()[before].clone() else { unreachable!() };
    let list = &embeds[0];
    assert_eq!((list.title.as_str(), list.description.as_str()), ("📝 Liste générale", "**4** à faire · **1** fait"));
    assert!(matches(r"^✅ 1. Ilias doit avoir", &list.fields[0].name), "{}", list.fields[0].name);
    assert!(matches(r"^\*Prime Ilias · \d\d\/\d\d", &list.fields[0].value), "{}", list.fields[0].value);
    assert!(matches(r"^⬜ 2. ", &list.fields[1].name), "{}", list.fields[1].name);
    assert!(matches(r"^Ta liste est vide\.$", &r.go(json!([{ "type": "todo_list", "list": "moi" }]), &alex).await)); // only done items left
    assert_eq!(r.go(json!([{ "type": "todo_move", "text": "quatrième", "to": "moi" }]), &wak).await, "Je déplace « Quatrième point » dans ta liste.");
    assert_eq!(r.todo().iter().find(|x| x["text"] == "Quatrième point").unwrap()["list"], "261531632589078528");
    assert_eq!(r.go(json!([{ "type": "todo_remove", "text": "3" }]), &wak).await, "Je supprime : Une voix différente pour Claude"); // numbers as posted, done items included
    assert!(matches("Je ne trouve pas", &r.go(json!([{ "type": "todo_remove", "text": "inexistant truc" }]), &wak).await));
    // Old format (plain strings) still reads
    r.brain.store.save("todo", &json!(["ancien point"])).unwrap();
    assert!(matches(r"1 point\. ancien point", &r.go(json!([{ "type": "todo_list" }]), &wak).await));
    let _ = w;
}

#[test]
fn notes() {
    // "retiens / note / il faudrait que tu / rajoute dans la liste" keep the full text, before the LLM
    let notes: [(&str, &str, &str); 10] = [
        ("il faut que tu retiennes pour la prochaine fois. On va vouloir mettre des fondus là, ça va pas.", "On va vouloir mettre des fondus là, ça va pas.", "general"),
        ("il faudrait que tu gardes l'historique des quelques dernières demandes qui t'ont été faites, parce que des fois tu ne comprends pas, et il faudrait que tu réessayes.", "Il faudrait que tu gardes l'historique des quelques dernières demandes qui t'ont été faites, parce que des fois tu ne comprends pas, et il faudrait que tu réessayes.", "general"),
        ("est-ce que tu peux noter dans la liste des choses à faire ? Il faut que tu aies absolument en connaissance, un beau contexte, que ce soit sur toi ce que tu es capable de faire, que ce soit sur le fait que tu puisses, par exemple, gérer une to-do list ou que tu puisses répondre à des questions te concernant.", "Il faut que tu aies absolument en connaissance, un beau contexte, que ce soit sur toi ce que tu es capable de faire, que ce soit sur le fait que tu puisses, par exemple, gérer une to-do list ou que tu puisses répondre à des questions te concernant.", "general"),
        ("tu peux rajouter à la to-do list que Ilias doit avoir absolument tous les droits à partir de maintenant sur le bot ?", "Ilias doit avoir absolument tous les droits à partir de maintenant sur le bot", "general"),
        ("j'aimerais que quand Claude me réponde, ça soit une voix différente. Est-ce que tu peux le noter dans la liste ?", "J'aimerais que quand Claude me réponde, ça soit une voix différente.", "general"),
        ("est-ce que tu peux noter dans ta liste que tu dois être capable d'éditer tes propres messages que tu as envoyés sur Discord ? Que ça soit les supprimer, les modifier, enfin voilà, tout ce qui est pour l'édit de tes messages personnels à toi.", "Tu dois être capable d'éditer tes propres messages que tu as envoyés sur Discord ? Que ça soit les supprimer, les modifier, enfin voilà, tout ce qui est pour l'édit de tes messages personnels à toi.", "general"),
        ("note qu'il faut une voix différente pour Claude", "Il faut une voix différente pour Claude", "general"),
        ("ajoute acheter du pain à ma liste", "Acheter du pain", "moi"),
        ("retiens que la soirée est samedi", "La soirée est samedi", "general"),
        ("add buy milk to my list", "Buy milk", "moi"),
    ];
    for (phrase, text, list) in notes {
        assert_eq!(quick(phrase), Some(vec![json!({ "type": "todo_add", "text": text, "list": list })]), "note: \"{phrase}\"");
    }
    for phrase in ["qu'est-ce que j'ai sur ma liste", "enlève le pain de ma liste", "joue Notre Dame", "mute Kévin"] {
        assert!(quick(phrase).map_or(true, |a| !a.iter().any(|x| x["type"] == "todo_add")), "not a note: \"{phrase}\"");
    }
    for phrase in ["montre-moi la liste de tout ce que tu as retenu.", "montre ce que tu as retenu", "affiche la liste"] {
        assert_eq!(quick(phrase), Some(vec![json!({ "type": "todo_list", "list": "general" })]), "{phrase}");
    }
    assert_eq!(quick("peux-tu afficher ma liste ?"), Some(vec![json!({ "type": "todo_list", "list": "moi" })]));
    assert_eq!(quick("qui a quel surnom").unwrap()[0]["type"], "alias_list");
}

#[test]
fn no_fake_noted() {
    // Never a fake "noté": a reply claiming it saved something, with no saving step, becomes the offer
    let r = rig_store();
    let wak = as_(Role::Super, "WakandeX");
    assert_eq!(note_guard("J'ai bien noté : on va vouloir mettre des fondus.", &[], "fr"), ("Tu veux que je l'ajoute à la liste ?".to_string(), true));
    assert_eq!(note_guard("Noted, I will remember that.", &[], "en"), ("Do you want me to add it to the list?".to_string(), true));
    assert_eq!(note_guard("Merci ! Tu veux que je l'ajoute à la liste ?", &[], "fr"), ("Merci ! Tu veux que je l'ajoute à la liste ?".to_string(), true));
    assert_eq!(note_guard("Il est 23 h.", &[], "fr"), ("Il est 23 h.".to_string(), false));
    let saving = resolve(&[json!({ "type": "todo_add", "text": "x" })], &wak, &r).unwrap();
    assert_eq!(note_guard("C'est noté dans la liste générale.", &saving, "fr"), ("C'est noté dans la liste générale.".to_string(), false));
}

/// A store alone, for the tests that never run a plan.
fn rig_store() -> Store {
    Store::temp()
}

#[test]
fn long_confirmation() {
    // Confirmation: a long yes passes, any "non" / "pas" cancels
    for t in ["Oui je confirme Bobby je confirme", "oui vas-y tu peux le déconnecter maintenant"] {
        assert!(confirms(t), "{t}");
    }
    for t in ["oui enfin non", "ok mais pas lui"] {
        assert!(!confirms(t), "{t}");
    }
}

#[test]
fn llm_guards() {
    // Guards on the LLM output
    let c = ctx();
    let g = |actions: Value, q: &str, history: &[Hist]| guard(json!({ "actions": actions, "say": "" }), q, &c, history, "fr")["actions"].clone();
    assert_eq!(g(json!([{ "type": "leave" }]), "BOBBY STOP", &[]), json!([{ "type": "music_stop", "quiet": true }]));
    assert_eq!(g(json!([{ "type": "leave" }]), "quitte le salon", &[]), json!([{ "type": "leave" }]));
    assert_eq!(g(json!([{ "type": "pc_launch", "text": "Apex Legends" }]), "ferme Apex Legends", &[]), json!([{ "type": "pc_close", "text": "Apex Legends" }]));
    assert_eq!(g(json!([{ "type": "alias_list" }]), "montre-moi la liste de tout ce que tu as retenu.", &[]), json!([{ "type": "todo_list", "list": "general" }]));
    assert_eq!(g(json!([{ "type": "admin_add", "who": ["Marie"] }]), "passe Marie super admin", &[]), json!([{ "type": "super_admin" }]));
    for q in [
        "est-ce que tu penses que tu es meilleur que Iris ?",
        "est-ce que tu peux expliquer à Ilias en une phrase, c'est quoi les permissions qu'il y a sur toi ?",
        "je trouve que ta détection n'est pas encore parfaite sur ton mode d'activation.",
    ] {
        assert_eq!(g(json!([{ "type": "ask", "text": q }]), q, &[])[0]["type"], "about", "{q}");
    }
    let past = [Hist { query: "lance Apex".into(), reply: "Je lance Apex Legends.".into(), ..Default::default() }];
    assert_eq!(g(json!([{ "type": "ask", "text": "x" }]), "Pourquoi il était perdu ?", &past)[0]["type"], "about");
    for q in ["c'est quoi la capitale de l'Australie", "est-ce que tu peux me dire la population de la République démocratique du Congo ?", "raconte-moi une blague", "pourquoi le ciel est bleu"] {
        assert!(!about_bobby(q, &c, &[]), "world question stays with Claude: \"{q}\"");
    }
    let long = "est-ce que tu peux noter dans la liste des choses à faire ? Il faut que tu aies absolument en connaissance, un beau contexte, que ce soit sur toi";
    let text = g(json!([{ "type": "todo_add", "text": "gérer une to-do list" }]), long, &[])[0]["text"].as_str().unwrap().to_string();
    assert!(matches(r"^Il faut que tu aies absolument en connaissance", &text), "{text}");
}

#[tokio::test]
async fn stop() {
    // Stop alone: stops the music if it plays, otherwise silence; leaving only on "pars"
    let stop = || Some(vec![json!({ "type": "music_stop", "text": "", "quiet": true })]);
    assert_eq!(quick("stop"), stop());
    assert_eq!(quick(&"Bobby stop".replacen("bobby", "", 1).trim_start().replacen("Bobby", "", 1).trim_start().to_string()), stop());
    for phrase in ["pars", "quitte le salon", "tu peux partir"] {
        assert_eq!(quick(phrase).unwrap()[0]["type"], "leave", "{phrase}");
    }
    let c = ctx();
    let busy = Arc::new(AtomicBool::new(false));
    let b2 = busy.clone();
    let r = rig_with(move |cap| match cap {
        Cap::MusicStop if b2.load(Ordering::SeqCst) => Ok(json!({ "stopped": { "title": "x" }, "cleared": 0 })),
        Cap::MusicStop => Ok(json!({ "stopped": null, "cleared": 0 })),
        _ => Ok(Value::Null),
    });
    let mut st = resolve(&quick("stop").unwrap(), &c, &r.brain.store).unwrap();
    r.run(&mut st, &c).await.unwrap(); // idle
    assert_eq!(describe(&st), "");
    busy.store(true, Ordering::SeqCst);
    let mut st2 = resolve(&quick("stop").unwrap(), &c, &r.brain.store).unwrap();
    r.run(&mut st2, &c).await.unwrap();
    assert_eq!(describe(&st2), "Musique arrêtée.");
}

#[tokio::test]
async fn close_app() {
    // Close an app or game (super admin), never mistaken for launching it
    let c = ctx();
    let r = rig();
    assert_eq!(quick("ferme Apex Legends"), Some(vec![json!({ "type": "pc_close", "text": "apex legends" })]));
    assert_eq!(quick("ferme le micro de Kévin").unwrap()[0]["type"], "mute");
    assert!(!quick("quitte le salon").unwrap().iter().any(|a| a["type"] == "pc_close"));
    let mut cl = resolve(&quick("ferme Apex Legends").unwrap(), &c, &r.brain.store).unwrap();
    assert_eq!(needed(&cl, None), Role::Super);
    r.run(&mut cl, &c).await.unwrap();
    // JS: the dry-run command line starts with taskkill.exe; here the brain asks the PC client to close the app
    assert_eq!(caps(&r.sent)[0], Cap::PcClose { name: "apex legends".into() });
    assert_eq!(describe(&cl), "Je ferme apex legends.");
}

#[tokio::test]
async fn super_admin_untouchable() {
    // Super admin untouchable: no promotion, no removal, by anyone
    let r = rig();
    let (c, wak) = (ctx(), as_(Role::Super, "WakandeX"));
    for phrase in ["passe Marie super admin", "nomme Lucas super admin", "retire le super admin à WakandeX"] {
        let mut p = resolve(&quick(phrase).unwrap(), &wak, &r.brain.store).unwrap();
        assert_eq!(p[0].kind, "super_admin", "{phrase}");
        let e = r.run(&mut p, &wak).await.expect_err(phrase).to_string();
        assert!(matches(r"fichier \.env", &e), "{e}");
    }
    let mut p = plan_of(&r, "retire l'admin à WakandeX", &c);
    let e = r.run(&mut p, &c).await.unwrap_err().to_string();
    assert!(matches("super admin", &e), "{e}");
    let mut p = plan_of(&r, "enlève les permissions de WakandeX", &c);
    let e = r.run(&mut p, &c).await.unwrap_err().to_string();
    assert!(matches("super admin", &e), "{e}");
    assert_eq!(role_of(WAKANDEX, &[], &r.brain.store), Role::Super);
    assert_eq!(role_of("Marie", &[], &r.brain.store), Role::Other);
}

#[tokio::test]
async fn self_knowledge_in_sync() {
    // Self-knowledge follows live state: rights of X before / after "passe X admin" and a blacklist
    let r = rig();
    let (c, alex) = (ctx(), as_(Role::Prime, "Alex"));
    let rights = |name: &'static str| {
        let (r, alex) = (&r, &alex);
        async move {
            let mut p = resolve(&quick(&format!("quels sont les droits de {name}")).unwrap(), alex, &r.brain.store).unwrap();
            r.run(&mut p, alex).await.unwrap();
            describe(&p)
        }
    };
    let facts = |step: Step| {
        let (r, c) = (&r, &c);
        async move {
            let env = Env { io: &r.brain.io, store: &r.brain.store, ctx: c, llm: &r.brain.llm, post_to: None };
            info::facts(&step, &env).await.unwrap()
        }
    };
    let by_alex = || Step { by: By { id: String::new(), name: "Alex".into(), role: Role::Prime }, ..Default::default() };
    let s = rights("Marie").await;
    assert!(matches(r"^Marie n'a aucun rôle", &s), "{s}");
    let mut p = plan_of(&r, "passe Marie admin", &c);
    r.run(&mut p, &c).await.unwrap();
    let s = rights("Marie").await;
    assert!(matches(r"^Marie est admin : .*la modération.*la musique", &s), "{s}");
    let s = facts(by_alex()).await;
    assert!(matches(r"Admins : .*Marie", &s), "{s}");
    let mut p = plan_of(&r, "enlève les permissions de Marie", &c);
    r.run(&mut p, &c).await.unwrap();
    let s = rights("Marie").await;
    assert!(matches(r"^Marie est sur liste noire", &s), "{s}");
    let s = facts(by_alex()).await;
    assert!(matches(r"liste noire\) : .*Marie", &s), "{s}");
    let all = facts(Step::default()).await;
    let line = all.lines().find(|l| l.starts_with("Super admin")).unwrap();
    assert!(!matches(r"Admins : [^.]*Marie", line), "{line}");
    let s = rights("WakandeX").await;
    assert!(matches(r"^WakandeX est super admin", &s), "{s}");
    assert_eq!(needed(&resolve(&quick("quels sont les droits de Lucas").unwrap(), &alex, &r.brain.store).unwrap(), None), Role::Prime);
}

#[test]
fn history() {
    // History: shared by everyone talking to Bobby, last 10 exchanges within 30 minutes (batch 4)
    let now = chrono::Utc::now().timestamp_millis();
    let at = |min: i64| iso_now_ms(now - min * 60_000);
    let rec = |v: Value| -> Map<String, Value> { v.as_object().unwrap().clone() };
    let mut m = Memory::new();
    m.remember(&rec(json!({ "at": at(31), "id": "h1", "who": "Old", "query": "trop vieux", "outcome": "done" })), now);
    for i in 1..=12 {
        let (id, who) = if i % 2 == 1 { ("h1", "Lucas") } else { ("h2", "Marie") };
        m.remember(&rec(json!({ "at": at(0), "id": id, "who": who, "query": format!("demande {i}"), "plan": "time", "reply": format!("réponse {i}"), "outcome": "done" })), now);
    }
    m.remember(&rec(json!({ "at": at(0), "id": "h1", "query": "Bobby", "outcome": "waiting" })), now); // not an exchange
    let queries: Vec<String> = m.conversation(now).into_iter().map(|h| h.query).collect();
    assert_eq!(queries, (0..10).map(|i| format!("demande {}", i + 3)).collect::<Vec<_>>());
    let first = &m.conversation(now)[0];
    assert_eq!((first.who.as_str(), first.did.as_str()), ("Lucas", "time"));
}

#[test]
fn own_messages() {
    // Own messages: clean needs a "oui", is admin, and defaults to the text channel
    let r = rig_store();
    let alex = as_(Role::Prime, "Alex");
    let cp = resolve(&quick("nettoie memes").unwrap(), &alex, &r).unwrap();
    assert_eq!((cp[0].kind.as_str(), cp[0].channel.as_ref().unwrap().name.as_str(), needed(&cp, None)), ("clean", "memes", Role::Admin));
    assert!(actions::def("clean").unwrap().confirm);
    assert_eq!(quick("supprime ton dernier message").unwrap()[0]["type"], "msg_delete");
}

#[tokio::test]
async fn review_fixes() {
    // After review: undo is per person and per item; "dernier" counts in the general list; claims in answers stay
    let r = rig();
    r.brain.store.save("todo", &json!([])).unwrap();
    let (alex, bob, wak) = (as_(Role::Prime, "Alex"), as_(Role::Prime, "Lucas"), as_(Role::Super, "WakandeX"));
    r.go(json!([{ "type": "todo_add", "text": "point de Alex" }]), &alex).await;
    r.go(json!([{ "type": "todo_add", "text": "point de Lucas" }]), &bob).await;
    r.try_run(&[json!({ "type": "undo" })], &bob).await.unwrap(); // Lucas undoes his own add only
    assert_eq!(r.texts(), ["point de Alex"]);
    let u2 = r.try_run(&[json!({ "type": "undo" })], &bob).await.unwrap();
    assert_eq!(describe(&u2), "Il n'y a rien à annuler.");
    assert_eq!(r.texts(), ["point de Alex"]); // Lucas cannot undo Alex's add
    r.go(json!([{ "type": "todo_add", "text": "perso de Lucas", "list": "moi" }]), &bob).await;
    r.go(json!([{ "type": "todo_remove", "text": "point de Alex" }]), &alex).await;
    r.go(json!([{ "type": "todo_add", "text": "encore Lucas" }]), &bob).await;
    let u3 = r.try_run(&[json!({ "type": "undo" })], &alex).await.unwrap(); // Alex's delete is undone, Lucas's later add survives
    assert_eq!(describe(&u3), "J'annule.");
    assert_eq!(r.texts(), ["point de Alex", "perso de Lucas", "encore Lucas"]);
    let s = r.go(json!([{ "type": "todo_remove", "text": "dernier" }]), &wak).await;
    assert!(matches("encore Lucas", &s), "{s}"); // not Lucas's personal item
    let ask = resolve(&[json!({ "type": "ask", "text": "x" })], &wak, &r.brain.store).unwrap();
    assert!(!note_guard("Les ingrédients sont ajoutés un par un.", &ask, "fr").1);
    assert!(confirms("Bobby. Oui."));
}

/// The end of `review_fixes`, on the state the undo step should have left.
#[tokio::test]
async fn review_fixes_rest() {
    let r = rig();
    r.brain.store.save("todo", &json!([])).unwrap();
    let (alex, bob, wak) = (as_(Role::Prime, "Alex"), as_(Role::Prime, "Lucas"), as_(Role::Super, "WakandeX"));
    r.go(json!([{ "type": "todo_add", "text": "point de Alex" }]), &alex).await;
    r.go(json!([{ "type": "todo_add", "text": "perso de Lucas", "list": "moi" }]), &bob).await;
    r.go(json!([{ "type": "todo_add", "text": "encore Lucas" }]), &bob).await;
    assert_eq!(r.texts(), ["point de Alex", "perso de Lucas", "encore Lucas"]);
    let s = r.go(json!([{ "type": "todo_remove", "text": "dernier" }]), &wak).await;
    assert!(matches("encore Lucas", &s), "{s}"); // not Lucas's personal item
    let ask = resolve(&[json!({ "type": "ask", "text": "x" })], &wak, &r.brain.store).unwrap();
    assert!(!note_guard("Les ingrédients sont ajoutés un par un.", &ask, "fr").1);
    assert!(confirms("Bobby. Oui."));
}
