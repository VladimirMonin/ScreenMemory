//! Actual bundled SQLite, populated production projection and shared SearchApi.
use super::*;
#[test]
fn s06_production_10k_latency_cancel_and_next_write() {
    rt().block_on(async{
 let(f,w)=fresh().await;w.shutdown(Duration::from_secs(5)).await.unwrap();let mut db=f.db();let rare=category(&mut db,"RareOnePercent");let mut ids=Vec::new();
 for n in 0..10000 {let id=KnowledgeId(Uuid::new_v4());seed(&mut db,id,"Benchmark searchable Q8_0",&format!("Synthetic benchmark bench_item_{n} settings.json"),if n%100==0{Some(rare)}else{None},1);ids.push(id);}
 let actual:u32=db.query_row("SELECT COUNT(*) FROM search_documents",[],|r|r.get(0)).unwrap();assert_eq!(actual,10000);drop(db);
 let w=f.start();let c=ctx(Uuid::new_v4(),1);let read=f.db();let mut sql_ms=Vec::new();let mut window_ms=Vec::new();let mut warm_ms=Vec::new();let mut rows=Vec::new();
 for n in 0..60 {
    let mut r=request(if n%3==0{"Benchmark"}else if n%3==1{"Q8_0"}else{"settings.json"});
    if n%2==0{r.filters.category=CategoryFilter::AnyOf{ids:vec![rare]};}
    let start=Instant::now();let candidates=retrieve(&read,&r,1).unwrap();sql_ms.push(start.elapsed().as_secs_f64()*1000.);
    let start=Instant::now();let result=page(&w,c.clone(),r.clone()).await;window_ms.push(start.elapsed().as_secs_f64()*1000.);
    let start=Instant::now();let replay=page(&w,c.clone(),r.clone()).await;warm_ms.push(start.elapsed().as_secs_f64()*1000.);
    assert_eq!(result.items.iter().map(|h|h.record_id).collect::<Vec<_>>(),replay.items.iter().map(|h|h.record_id).collect::<Vec<_>>());
    assert_eq!(candidates.len(),if n%2==0{100}else{101});
    rows.push(serde_json::json!({"query":r.text,"category":"rare1pct_or_any","candidate_count":candidates.len(),"window_truncated":result.window_truncated}));
 }
 drop(read);
 let mut hold=Vec::new();for n in 0..30 {let request=KnowledgeRequest::Edit(CardEdit{operation_id:KnowledgeId(Uuid::new_v4()),record_id:ids[0],expected_record_revision:SqlRevision(2+n),expected_catalog_revision:SqlRevision(2),expected_category_revision:None,patch:CardPatch{description:FieldPatch::Set{value:CardDescription(format!("Mutation bench_rev_{n}"))},..Default::default()}});let start=Instant::now();w.knowledge(c.clone(),request).await.unwrap();hold.push(start.elapsed().as_secs_f64()*1000.);}
 let cancel=CancellationToken::new();let child=cancel.clone();let w2=w.clone();let c2=c.clone();let task=tokio::spawn(async move{w2.search(c2,SearchRequest::Start(request("Benchmark Q8_0 settings.json searchable Synthetic benchmark")),child).await});tokio::time::sleep(Duration::from_millis(1)).await;cancel.cancel();let outcome=task.await.unwrap();assert!(outcome.is_err(),"actual in-flight cancellation must not be empty success");
 w.knowledge(c.clone(),KnowledgeRequest::Edit(CardEdit{operation_id:KnowledgeId(Uuid::new_v4()),record_id:ids[0],expected_record_revision:SqlRevision(32),expected_catalog_revision:SqlRevision(2),expected_category_revision:None,patch:CardPatch{title:FieldPatch::Set{value:CardTitle("NextOrdinarySave99".into())},..Default::default()}})).await.unwrap();assert_eq!(page(&w,c,request("NextOrdinarySave99")).await.items[0].record_id,ids[0]);
 fn stats(v:&mut[f64])->serde_json::Value{v.sort_by(f64::total_cmp);serde_json::json!({"p50_ms":v[v.len()/2],"p95_ms":v[(v.len()*95/100).min(v.len()-1)],"n":v.len()})}
 let sql=stats(&mut sql_ms);let window=stats(&mut window_ms);let warm=stats(&mut warm_ms);let hold=stats(&mut hold);
 let value=serde_json::json!({"records":actual,"sqlite":"rusqlite0.37_bundled_production","sql":sql,"new_window":window,"warm_replay":warm,"writer_request_elapsed_upper_bound_hold":hold,"cold":"first sample only; no OS cache flush authority","readers":2,"queue":32,"inflight_cancel":"refused","next_ordinary_save":"passed","samples":rows});
 let output=Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary").join(Uuid::new_v4().to_string());fs::create_dir(&output).unwrap();fs::write(output.join("production-10k.json"),serde_json::to_vec_pretty(&value).unwrap()).unwrap();println!("10k evidence UUID={} sql={} window={} warm={} hold={}",output.file_name().unwrap().to_string_lossy(),sql,window,warm,hold);
 w.shutdown(Duration::from_secs(5)).await.unwrap();
 assert!(sql["p95_ms"].as_f64().unwrap()<=100.);assert!(warm["p95_ms"].as_f64().unwrap()<=300.);assert!(hold["p95_ms"].as_f64().unwrap()<=100.);
});
}
