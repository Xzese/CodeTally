#![cfg(unix)]
use codetally_lib::{db::Database, github_sync, models::Repository};
use serde_json::{json, Value};
use std::{os::unix::fs::PermissionsExt, path::PathBuf, sync::Mutex};
static ENV: Mutex<()> = Mutex::new(());

struct Fixture { root: PathBuf, old_path: Option<std::ffi::OsString>, db: Database, repo: Repository }
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("personal-search-{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap()));
        std::fs::create_dir_all(&root).unwrap();
        let db = Database::new(root.join("db.sqlite")); db.init().unwrap();
        db.upsert_repository(&Repository { github_id:"R1".into(), owner:"me".into(), name:"one".into(), name_with_owner:"me/one".into(), ..Repository::default() }).unwrap();
        let repo = db.repositories().unwrap().remove(0);
        db.set_open_counts(repo.id, 42, 43).unwrap();
        let script = root.join("gh");
        std::fs::write(&script, format!(r#"#!/usr/bin/python3
import sys,json,pathlib
root=pathlib.Path({root:?})
args=sys.argv[1:]
with (root/'calls').open('a') as f: f.write(json.dumps(args)+'\n')
if (root/'dataset').exists():
 values=dict(arg.split('=',1) for arg in args if '=' in arg)
 search=values['search']
 low,high=next(word[8:] for word in search.split() if word.startswith('updated:')).split('..')
 nodes=json.loads((root/'dataset').read_text()) if 'is:pr author:me ' in search else []
 nodes=[node for node in nodes if low <= node['updatedAt'] <= high]
 offset=0 if values['after']=='null' else int(values['after'])
 end=min(offset+100,len(nodes),1000)
 response={{'data':{{'search':{{'issueCount':len(nodes),'nodes':nodes[offset:end],'pageInfo':{{'hasNextPage':end<min(len(nodes),1000),'endCursor':str(end)}}}}}}}}
 print(json.dumps(response)); sys.exit(0)
i=int((root/'index').read_text()) if (root/'index').exists() else 0
(root/'index').write_text(str(i+1))
response=json.loads((root/'responses').read_text())[i]
if isinstance(response,str):
 print(response,file=sys.stderr); sys.exit(1)
print(json.dumps(response))
"#, root=root.to_str().unwrap())).unwrap();
        std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let old_path = std::env::var_os("PATH");
        let mut paths = vec![root.clone()]; paths.extend(std::env::split_paths(old_path.as_deref().unwrap_or_default()));
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
        Self { root, old_path, db, repo }
    }
    fn responses(&self, responses: Vec<Value>) { std::fs::write(self.root.join("responses"), json!(responses).to_string()).unwrap(); std::fs::write(self.root.join("index"), "0").unwrap(); }
    fn run(&self) -> codetally_lib::error::AppResult<github_sync::PersonalSyncReport> { github_sync::sync_personal_activity(&self.db, "Me", &[self.repo.clone()]) }
    fn calls(&self) -> Vec<Vec<String>> { std::fs::read_to_string(self.root.join("calls")).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect() }
    fn cursor_key(&self) -> &'static str { "github_personal_v1:github.com:me:[\"R1\"]:author:pr" }
}
impl Drop for Fixture { fn drop(&mut self) { match &self.old_path { Some(path)=>std::env::set_var("PATH",path),None=>std::env::remove_var("PATH") }; let _=std::fs::remove_dir_all(&self.root); } }
fn item(id:&str, repo:&str, number:i64, state:&str) -> Value { json!({"id":id,"repository":{"id":repo},"number":number,"title":"Personal work","state":state,"isDraft":false,"createdAt":"2026-01-01T00:00:00Z","updatedAt":chrono::Utc::now().to_rfc3339(),"closedAt":if state=="OPEN" {None} else {Some("2026-09-01T00:00:00Z")},"mergedAt":if state=="MERGED" {Some("2026-09-01T00:00:00Z")} else {None},"stateReason":"COMPLETED","url":"https://github.com/me/one/issues/1","author":{"login":"me"},"labels":{"nodes":[]},"assignees":{"nodes":[{"login":"me"}]}}) }
fn page(nodes:Vec<Value>, after:Option<&str>) -> Value { json!({"data":{"search":{"issueCount":nodes.len(),"nodes":nodes,"pageInfo":{"hasNextPage":after.is_some(),"endCursor":after}}}}) }
fn empty() -> Value { page(vec![],None) }

#[test]
fn pages_deduplicate_filter_repositories_preserve_states_and_overlap() {
    let _lock=ENV.lock().unwrap(); let f=Fixture::new();
    codetally_lib::sync::save_app_settings(&f.db, &codetally_lib::models::AppSettings { kanban_enabled:true, ..Default::default() }).unwrap();
    let pr=item("P1","R1",1,"MERGED"); let issue=item("I2","R1",2,"CLOSED");
    f.responses(vec![page(vec![pr.clone(),item("X","R2",9,"OPEN")],Some("next")),empty(),page(vec![issue.clone()],None),page(vec![pr],None),page(vec![issue],None)]);
    let report=f.run().unwrap(); assert!(report.complete); assert_eq!((report.pull_requests,report.issues),(1,1));
    let prs=f.db.pull_requests(None,None,100).unwrap(); assert_eq!(prs.len(),1); assert_eq!(prs[0].state,"MERGED"); assert!(prs[0].merged_at.is_some());
    assert_eq!(f.db.issues(None,None,100).unwrap()[0].state,"CLOSED");
    let repo=f.db.repositories().unwrap().remove(0); assert_eq!((repo.open_pr_count,repo.open_issue_count),(42,43));
    let conn=rusqlite::Connection::open(f.root.join("db.sqlite")).unwrap();
    let identity:(String,String)=conn.query_row("SELECT node_id,completion_reason FROM issues WHERE number=2",[],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
    assert_eq!(identity,("I2".into(),"COMPLETED".into()));
    let calls=f.calls(); assert!(calls[1].contains(&"after=next".into()));
    let checkpoint:Value=serde_json::from_str(&f.db.metadata(f.cursor_key()).unwrap().unwrap()).unwrap();
    let completed=checkpoint["completed_at"].as_str().unwrap().parse::<chrono::DateTime<chrono::Utc>>().unwrap();
    f.responses(vec![empty(),empty(),empty(),empty()]); f.run().unwrap();
    let calls=f.calls(); let cutoff=(completed-chrono::Duration::minutes(5)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
    assert!(calls[5].iter().any(|arg|arg.contains(&format!("updated:{cutoff}.."))));
}

#[test]
fn bounded_pages_resume_and_errors_preserve_cursor_and_cache() {
    let _lock=ENV.lock().unwrap(); let f=Fixture::new();
    let mut responses=Vec::new(); for n in 0..5 { responses.push(page(vec![item(&format!("P{n}"),"R1",n,"OPEN")],Some(&format!("c{n}")))); }
    responses.extend([empty(),empty(),empty()]); f.responses(responses);
    assert!(!f.run().unwrap().complete); assert_eq!(f.db.pull_requests(None,None,100).unwrap().len(),5);
    f.responses(vec![page(vec![item("P5","R1",5,"OPEN")],Some("c5")),json!("permanent server failure")]); assert!(f.run().is_err());
    let cursor:Value=serde_json::from_str(&f.db.metadata(f.cursor_key()).unwrap().unwrap()).unwrap(); assert_eq!(cursor["after"],"c5"); assert!(cursor["completed_at"].is_null());
    f.responses(vec![json!("secondary rate limit")]); assert!(matches!(f.run(),Err(codetally_lib::error::AppError::RateLimited{..})));
    let calls=f.calls().len(); assert!(f.run().is_err()); assert_eq!(calls,f.calls().len());
    assert_eq!(f.db.pull_requests(None,None,100).unwrap().len(),6);
    f.db.set_metadata(github_sync::PAUSE_KEY,"2000-01-01T00:00:00Z").unwrap();
    f.responses(vec![empty(),empty(),empty(),empty()]); assert!(f.run().unwrap().complete);
    assert!(f.calls()[calls].contains(&"after=c5".into()));
}

#[test]
fn oversized_search_splits_time_windows_and_resumes_until_all_items_are_saved() {
    let _lock=ENV.lock().unwrap(); let f=Fixture::new();
    let started=chrono::DateTime::from_timestamp(chrono::Utc::now().timestamp(),0).unwrap();
    let middle=started-chrono::Duration::days(15);
    let format_time=|time:chrono::DateTime<chrono::Utc>|time.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let mut nodes=Vec::new();
    for n in 0..1001 {
        let mut node=item(&format!("P{n}"),"R1",n,"MERGED");
        // Both sides of the split boundary must be imported exactly once.
        node["updatedAt"]=json!(format_time(if n<500 {middle} else {middle+chrono::Duration::seconds(1)}));
        nodes.push(node);
    }
    std::fs::write(f.root.join("dataset"),json!(nodes).to_string()).unwrap();
    // A legacy unbounded cursor must replay safely using new bounded windows.
    f.db.set_metadata(f.cursor_key(), &json!({"started_at":started.to_rfc3339(),"completed_at":null,"after":"legacy-cursor"}).to_string()).unwrap();
    let first=f.run().unwrap(); assert!(!first.complete); assert_eq!(first.pull_requests,400);
    let saved:Value=serde_json::from_str(&f.db.metadata(f.cursor_key()).unwrap().unwrap()).unwrap();
    assert!(saved["completed_at"].is_null()); assert_eq!(saved["windows"].as_array().unwrap().len(),2);
    assert_eq!(f.calls().len(),8); // Five search requests plus the three empty streams.
    let second=f.run().unwrap(); assert!(!second.complete); assert_eq!(second.pull_requests,500);
    let third=f.run().unwrap(); assert!(third.complete); assert_eq!(third.pull_requests,101);
    assert_eq!(f.db.pull_requests(None,None,2000).unwrap().len(),1001);
    let saved:Value=serde_json::from_str(&f.db.metadata(f.cursor_key()).unwrap().unwrap()).unwrap();
    assert_eq!(saved["completed_at"],started.to_rfc3339()); assert!(saved["windows"].as_array().unwrap().is_empty());
    let calls=f.calls();
    let requests:Vec<_>=calls.iter().filter_map(|args|args.iter().find(|arg|arg.starts_with("search=is:pr author:me "))).collect();
    assert_eq!(requests.len(),12); // One split probe and eleven actual pages.
    let left=format!("updated:{}..{}",format_time(started-chrono::Duration::days(30)),format_time(middle));
    let right=format!("updated:{}..{}",format_time(middle+chrono::Duration::seconds(1)),format_time(started));
    assert!(requests[1].contains(&left)); assert!(requests[6].contains(&right));
    assert!(calls[0].contains(&"after=null".into()));
}

#[test]
fn single_second_saturation_keeps_checkpoint_and_stops_repeating_api_requests() {
    let _lock=ENV.lock().unwrap(); let f=Fixture::new();
    let completed="2026-09-01T00:00:00Z";
    f.db.set_metadata(f.cursor_key(),&json!({"completed_at":completed,"started_at":"2026-09-02T00:00:00Z","after":null,"windows":[{"start":"2026-09-01T12:00:00Z","end":"2026-09-01T12:00:00Z"}]}).to_string()).unwrap();
    let mut crowded=empty(); crowded["data"]["search"]["issueCount"]=json!(1001);
    f.responses(vec![crowded,page(vec![item("I1","R1",1,"OPEN")],None),empty(),empty()]);
    assert!(f.run().unwrap_err().to_string().contains("single second"));
    assert_eq!(f.db.issues(None,None,10).unwrap()[0].state,"OPEN");
    let saved:Value=serde_json::from_str(&f.db.metadata(f.cursor_key()).unwrap().unwrap()).unwrap();
    assert_eq!(saved["completed_at"],completed); assert_eq!(saved["saturated"],true);
    let calls=f.calls().len();
    f.responses(vec![page(vec![item("I1","R1",1,"CLOSED")],None),empty(),empty()]);
    assert!(f.run().unwrap_err().to_string().contains("automatic retries are stopped"));
    assert_eq!(f.calls().len(),calls+3);
    assert_eq!(f.db.issues(None,None,10).unwrap()[0].state,"CLOSED");
    assert!(f.calls()[calls..].iter().all(|args| !args.iter().any(|arg|arg.starts_with("search=is:pr author:me "))));
}

#[test]
fn cached_assignments_are_reconciled_in_bounded_batches_after_removal() {
    let _lock=ENV.lock().unwrap(); let f=Fixture::new();
    // Simulate a legacy cache, with Kanban disabled and no persisted item node IDs.
    // The stale items are older than the initial search window and no longer match
    // author:me or assignee:me after GitHub removes the assignment.
    for number in 1..=51 {
        f.db.upsert_pull_request(&codetally_lib::models::PullRequest {
            repository_id:f.repo.id,number,author:Some("other".into()),assignees:vec!["Me".into()],
            title:"Old assignment".into(),state:"OPEN".into(),created_at:"2025-01-01T00:00:00Z".into(),updated_at:"2025-01-01T00:00:00Z".into(),
            ..Default::default()
        }).unwrap();
    }
    f.db.upsert_issue(&codetally_lib::models::Issue {
        repository_id:f.repo.id,number:52,author:Some("other".into()),assignees:vec!["Me".into()],
        title:"Old issue assignment".into(),state:"OPEN".into(),created_at:"2025-01-01T00:00:00Z".into(),updated_at:"2025-01-01T00:00:00Z".into(),
        ..Default::default()
    }).unwrap();
    f.responses(vec![json!("permanent read failure")]); assert!(f.run().is_err());
    assert!(f.db.pull_requests(None,None,100).unwrap().iter().all(|pr|pr.assignees==["Me"]));
    let batch=|numbers:Vec<(i64,bool)>| {
        let mut data=serde_json::Map::new();
        for (index,(number,is_issue)) in numbers.into_iter().enumerate() {
            let mut node=item(&format!("N{number}"),"R1",number,if is_issue {"CLOSED"} else {"MERGED"});
            node["author"]=json!({"login":"other"}); node["assignees"]=json!({"nodes":[]});
            data.insert(format!("r{index}"),json!({"item":node}));
        }
        json!({"data":data})
    };
    let first_batch=std::iter::once((52,true)).chain((1..50).map(|number|(number,false))).collect();
    f.responses(vec![batch(first_batch),empty(),empty(),empty(),empty()]);
    let first=f.run().unwrap(); assert!(!first.complete); assert_eq!((first.pull_requests,first.issues),(49,1));
    assert!(f.db.issues(None,None,10).unwrap()[0].assignees.is_empty());
    assert_eq!(f.db.pull_requests(None,None,100).unwrap().iter().filter(|pr|!pr.assignees.is_empty()).count(),2);
    let assignment_key="github_personal_assignments_v1:github.com:me:[\"R1\"]";
    assert_eq!(serde_json::from_str::<Value>(&f.db.metadata(assignment_key).unwrap().unwrap()).unwrap()["after"],json!([f.repo.id,"pr",49]));
    f.responses(vec![batch(vec![(50,false),(51,false)]),empty(),empty(),empty(),empty()]);
    let second=f.run().unwrap(); assert!(second.complete); assert_eq!(second.pull_requests,2);
    assert!(f.db.pull_requests(None,None,100).unwrap().iter().all(|pr|pr.assignees.is_empty() && pr.state=="MERGED"));
    assert!(serde_json::from_str::<Value>(&f.db.metadata(assignment_key).unwrap().unwrap()).unwrap()["after"].is_null());
    let before=f.calls().len(); f.responses(vec![empty(),empty(),empty(),empty()]); assert!(f.run().unwrap().complete);
    assert_eq!(f.calls().len()-before,4); // Unassigned rows no longer need reconciliation.
}

#[test]
fn unresolved_assignment_keeps_later_batches_partial_until_a_retry_resolves_it() {
    let _lock=ENV.lock().unwrap(); let f=Fixture::new();
    for number in 1..=51 {
        f.db.upsert_pull_request(&codetally_lib::models::PullRequest {
            repository_id:f.repo.id,number,author:Some("other".into()),assignees:vec!["me".into()],
            title:"Cached assignment".into(),state:"OPEN".into(),created_at:"2025-01-01T00:00:00Z".into(),updated_at:"2025-01-01T00:00:00Z".into(),
            ..Default::default()
        }).unwrap();
    }
    let unassigned=|number| {
        let mut node=item(&format!("N{number}"),"R1",number,"MERGED");
        node["author"]=json!({"login":"other"}); node["assignees"]=json!({"nodes":[]}); node
    };
    let mut first=serde_json::Map::new(); first.insert("r0".into(),json!({"item":null}));
    for number in 2..=50 { first.insert(format!("r{}",number-1),json!({"item":unassigned(number)})); }
    f.responses(vec![json!({"data":first}),empty(),empty(),empty(),empty()]);
    assert!(!f.run().unwrap().complete);
    let key="github_personal_assignments_v1:github.com:me:[\"R1\"]";
    let saved:Value=serde_json::from_str(&f.db.metadata(key).unwrap().unwrap()).unwrap();
    assert_eq!(saved["after"],json!([f.repo.id,"pr",50])); assert_eq!(saved["unresolved"],true);
    f.responses(vec![json!({"data":{"r0":{"item":unassigned(51)}}}),empty(),empty(),empty(),empty()]);
    let second=f.run().unwrap(); assert_eq!(second.pull_requests,1); assert!(!second.complete);
    assert_eq!(f.db.pull_requests(None,None,100).unwrap().iter().filter(|pr|!pr.assignees.is_empty()).count(),1);
    // The next sweep revisits the unresolved first row, and remains incomplete
    // while it is unavailable, even though all other rows have been reconciled.
    f.responses(vec![json!({"data":{"r0":{"item":null}}}),empty(),empty(),empty(),empty()]);
    assert!(!f.run().unwrap().complete);
    f.responses(vec![json!({"data":{"r0":{"item":unassigned(1)}}}),empty(),empty(),empty(),empty()]);
    assert!(f.run().unwrap().complete);
    assert!(f.db.pull_requests(None,None,100).unwrap().iter().all(|pr|pr.assignees.is_empty()));
}
