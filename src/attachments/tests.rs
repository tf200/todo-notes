use super::*;

fn att(id: &str, name: &str) -> Attachment {
    Attachment { id: id.into(), name: name.into(), mime: "text/plain".into(), size: 3, created_at: String::new() }
}

fn task(id: &str) -> Owner {
    (Parent::Task, id.into())
}

#[test]
fn safe_names() {
    assert_eq!(safe_name("report.pdf"), "report.pdf");
    assert_eq!(safe_name("a/b\\c:d*e?.txt"), "a_b_c_d_e_.txt");
    assert_eq!(safe_name("CON.txt"), "_CON.txt");
    assert_eq!(safe_name("com1"), "_com1");
    assert_eq!(safe_name("console.log"), "console.log");
    assert_eq!(safe_name("trailing. . "), "trailing");
    assert_eq!(safe_name(""), "_");
    assert_eq!(safe_name("résumé 🎉.png"), "résumé 🎉.png");
}

#[test]
fn sizes() {
    assert_eq!(size_label(12), "12 bytes");
    assert_eq!(size_label(340 * 1024), "340 KB");
    assert_eq!(size_label(1_258_291), "1.2 MB");
}

#[test]
fn programs_are_not_run() {
    assert!(runs_as_program(Path::new("setup.EXE")));
    assert!(runs_as_program(Path::new("x/run.ps1")));
    assert!(!runs_as_program(Path::new("photo.jpg")));
    assert!(!runs_as_program(Path::new("README")));
}

#[test]
fn checks_files_before_uploading() {
    let dir = std::env::temp_dir().join(format!("todo-notes-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let empty = dir.join("empty.txt");
    std::fs::write(&empty, b"").unwrap();
    let ok = dir.join("ok.txt");
    std::fs::write(&ok, b"hello").unwrap();

    assert_eq!(check_file(&ok), Ok(("ok.txt".into(), 5)));
    assert!(check_file(&empty).unwrap_err().1.contains("empty"));
    assert!(check_file(&dir).unwrap_err().1.contains("Folders"));
    assert!(check_file(&dir.join("missing.txt")).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_upload_lands_in_its_list() {
    let mut f = Files::default();
    f.lists.insert(task("t1"), Load::Ready(vec![att("a1", "one.txt")]));
    let (key, sent) = f.begin_upload(task("t1"), "two.txt".into(), 10);
    sent.store(5, Ordering::Relaxed);
    assert_eq!(f.uploads[0].progress(), 0.5);

    assert!(!f.apply(Reply::Uploaded { key, result: Ok(att("a2", "two.txt")) }));
    assert!(f.uploads.is_empty());
    let Some(Load::Ready(list)) = f.of(&task("t1")) else { panic!() };
    assert_eq!(list.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["one.txt", "two.txt"]);
}

#[test]
fn a_failed_upload_stays_until_dismissed() {
    let mut f = Files::default();
    let (key, _) = f.begin_upload(task("t1"), "big.bin".into(), 10);
    assert!(!f.apply(Reply::Uploaded { key, result: Err(Failure::Api(ApiError::StorageFull)) }));
    assert_eq!(f.uploads[0].failed.as_deref(), Some(ApiError::StorageFull.message()));
    f.dismiss_upload(key);
    assert!(f.uploads.is_empty());

    // An ended sign-in is reported back.
    let (key, _) = f.begin_upload(task("t1"), "x".into(), 1);
    assert!(f.apply(Reply::Uploaded { key, result: Err(Failure::Api(ApiError::Unauthorized)) }));
}

#[test]
fn a_failed_refresh_keeps_the_list() {
    let mut f = Files::default();
    f.apply(Reply::Listed { owner: task("t1"), result: Ok(vec![att("a1", "one.txt")]) });
    f.apply(Reply::Listed { owner: task("t1"), result: Err(ApiError::Unreachable) });
    assert!(matches!(f.of(&task("t1")), Some(Load::Ready(l)) if l.len() == 1));

    f.apply(Reply::Listed { owner: task("t2"), result: Err(ApiError::Unreachable) });
    assert!(matches!(f.of(&task("t2")), Some(Load::Failed(ApiError::Unreachable))));
}

#[test]
fn removing() {
    let mut f = Files::default();
    f.lists.insert(task("t1"), Load::Ready(vec![att("a1", "one.txt"), att("a2", "two.txt")]));
    f.busy.insert("a1".into());
    f.apply(Reply::Removed { owner: task("t1"), id: "a1".into(), result: Ok(()) });
    assert!(f.busy.is_empty());
    assert!(matches!(f.of(&task("t1")), Some(Load::Ready(l)) if l.len() == 1));

    // Already gone on the server: removed here too, no complaint.
    f.apply(Reply::Removed { owner: task("t1"), id: "a2".into(), result: Err(ApiError::NotFound) });
    assert!(matches!(f.of(&task("t1")), Some(Load::Ready(l)) if l.is_empty()));
    assert_eq!(f.notice, None);

    f.lists.insert(task("t1"), Load::Ready(vec![att("a3", "three.txt")]));
    f.apply(Reply::Removed { owner: task("t1"), id: "a3".into(), result: Err(ApiError::Unreachable) });
    assert!(matches!(f.of(&task("t1")), Some(Load::Ready(l)) if l.len() == 1));
    assert!(f.notice.is_some());
}

#[test]
fn only_saved_items_take_files() {
    assert!(attachable(&task("6af1be8796eff59c477877d3")));
    assert!(!attachable(&task("tmp-3")));
}

mod live {
    //! Against the dev stack, with the dev account: `cargo test -- --ignored`, with
    //! `TODO_DEV_EMAIL` and `TODO_DEV_PASSWORD` set (see `.dev/credentials.env`).
    use super::*;
    use crate::session::NoStore;

    #[test]
    #[ignore = "needs the backend (with its file store) running and the dev account"]
    fn upload_download_and_remove_through_the_file_store() {
        let base = std::env::var("TODO_API_URL").unwrap_or_else(|_| crate::api::DEV_BASE_URL.into());
        let api = Api::new(base);
        let tokens = api
            .login(&std::env::var("TODO_DEV_EMAIL").unwrap(), &std::env::var("TODO_DEV_PASSWORD").unwrap())
            .expect("dev login");
        let creds = Creds::new(tokens, Arc::new(api.clone()), Arc::new(NoStore));
        let note = creds.call(|t| api.create_note(t, "Attachment e2e", "")).unwrap();
        let owner: Owner = (Parent::Note, note.id.clone());

        // A file a bit over a megabyte, with a name that needs escaping in a download header.
        let dir = std::env::temp_dir().join(format!("todo-notes-live-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("résumé; final (v2).bin");
        let bytes: Vec<u8> = (0..1_300_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &bytes).unwrap();

        let transfers = Transfers::start(api.clone(), Arc::new(|| {}));
        let sent = Arc::new(AtomicU64::new(0));
        let up = upload(&api, &transfers.store, &creds, &owner, &path, sent.clone(), &transfers.waker).expect("upload");
        assert_eq!(sent.load(Ordering::Relaxed), bytes.len() as u64);
        assert_eq!((up.name.as_str(), up.size, up.mime.as_str()), ("résumé; final (v2).bin", bytes.len() as u64, "application/octet-stream"));

        let listed = creds.call(|t| api.attachments(t, Parent::Note, &note.id)).unwrap();
        assert_eq!(listed.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), [up.id.as_str()]);

        let _ = std::fs::remove_dir_all(cache_dir().join(&up.id));
        let got = cached(&api, &transfers.store, &creds, &up).expect("download");
        assert_eq!(std::fs::read(&got).unwrap(), bytes);
        assert_eq!(got.file_name().unwrap().to_string_lossy(), "résumé; final (v2).bin");

        // Too big is refused before anything is sent.
        assert_eq!(
            creds.call(|t| api.start_attachment(t, Parent::Note, &note.id, "big.bin", ATTACHMENT_MAX + 1, "")),
            Err(ApiError::TooLarge)
        );

        // Deleting the note takes its files with it.
        creds.call(|t| api.delete_note(t, &note.id)).unwrap();
        assert_eq!(creds.call(|t| api.attachment_link(t, &up.id)), Err(ApiError::NotFound));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(cache_dir().join(&up.id));
    }
}
