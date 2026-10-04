use super::*;
#[test]
fn s06_frozen_graded_supplement_shared_service() {
    rt().block_on(async {
    let bytes=include_bytes!("../../../../tests/fixtures/search/graded-supplement-v1.json");
    let declared:serde_json::Value=serde_json::from_slice(bytes).unwrap();
    let (f,w)=fresh().await;w.shutdown(Duration::from_secs(5)).await.unwrap();let mut db=f.db();
    for r in declared["records"].as_array().unwrap(){seed(&mut db,KnowledgeId(Uuid::parse_str(r["id"].as_str().unwrap()).unwrap()),r["title"].as_str().unwrap(),r["description"].as_str().unwrap(),None,1);}
    drop(db);let w=f.start();let c=ctx(Uuid::new_v4(),1);let mut outputs=Vec::new();let mut failures=Vec::new();let mut recall=0.0;let mut ndcg=0.0;let mut positives=0;let mut exact_top=0;
    let key=|id:KnowledgeId|id.0.to_string().rsplit('-').next().unwrap().parse::<u32>().unwrap().to_string();
    for q in declared["queries"].as_array().unwrap(){
        let p=page(&w,c.clone(),request(q["text"].as_str().unwrap())).await;
        let ids=p.items.iter().map(|h|key(h.record_id)).collect::<Vec<_>>();
        let grades=q["grades"].as_object().unwrap();
        let forbidden=q["forbidden"].as_array().unwrap();
        if ids.iter().any(|id|forbidden.iter().any(|f|f.as_str()==Some(id))){failures.push(format!("{} forbidden",q["id"]));}
        if let Some(top)=q["top1"].as_str(){if ids.first().map(String::as_str)!=Some(top){failures.push(format!("{} top1",q["id"]));}else{exact_top+=1;}}
        if !grades.is_empty(){
            positives+=1;recall+=ids.iter().take(5).filter(|id|grades.contains_key(*id)).count() as f64/grades.len() as f64;
            let dcg=ids.iter().take(5).enumerate().map(|(i,id)|((2_f64.powf(grades.get(id).and_then(|g|g.as_f64()).unwrap_or(0.0)))-1.0)/(i as f64+2.0).log2()).sum::<f64>();
            let mut ideal=grades.values().map(|g|g.as_u64().unwrap()).collect::<Vec<_>>();ideal.sort_by(|a,b|b.cmp(a));
            let ideal=ideal.iter().take(5).enumerate().map(|(i,g)|(2_f64.powi(*g as i32)-1.0)/(i as f64+2.0).log2()).sum::<f64>();ndcg+=dcg/ideal;
        }else if !ids.is_empty(){failures.push(format!("{} negative",q["id"]));}
        outputs.push(serde_json::json!({"query_id":q["id"],"text":q["text"],"actual_ids":p.items.iter().map(|h|h.record_id).collect::<Vec<_>>(),"actual_keys":ids,"grades":grades}));
    }
    w.shutdown(Duration::from_secs(5)).await.unwrap();
    let output=f.0.parent().unwrap().join(Uuid::new_v4().to_string());fs::create_dir(&output).unwrap();
    use sha2::Digest;
    fs::write(output.join("graded-results.json"),serde_json::to_vec_pretty(&serde_json::json!({"fixture_sha256":format!("{:x}",sha2::Sha256::digest(bytes)),"queries":outputs,"positives":positives,"recall_at_5":recall/positives as f64,"ndcg_at_5":ndcg/positives as f64,"top1_pass":exact_top,"failures":failures,"limits":"one separately frozen literal supplement; not semantic/universal quality; core rarecategory/scope proof separate"})).unwrap()).unwrap();
    println!("graded evidence={}",output.file_name().unwrap().to_string_lossy());
    assert!(failures.is_empty(),"frozen expected policy failure: {failures:?}");assert!(recall/positives as f64>=0.85);assert!(ndcg/positives as f64>=0.95);
});
}
