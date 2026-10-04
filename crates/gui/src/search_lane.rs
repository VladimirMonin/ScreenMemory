//! One admitted search per window. Cancellation never waits behind GuiSession.
use crate::session::GuiSession;
use sm_protocol::{
    knowledge::KnowledgeId,
    search::{SearchReply, SearchRequest},
};
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct State {
    active: Option<(u64, Option<KnowledgeId>, CancellationToken)>,
    last: Option<KnowledgeId>,
    serial: u64,
}
#[derive(Clone, Default)]
pub struct SearchLane(Arc<Mutex<State>>);
struct Ticket {
    lane: SearchLane,
    serial: u64,
}
impl Drop for Ticket {
    fn drop(&mut self) {
        let mut state = self.lane.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.active.as_ref().is_some_and(|v| v.0 == self.serial) {
            state.active = None;
        }
    }
}
impl SearchLane {
    fn begin(&self, request: &SearchRequest) -> Result<(Ticket, CancellationToken), String> {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if s.active.is_some() {
            return Err("SEARCH_BUSY".into());
        }
        s.serial = s.serial.wrapping_add(1);
        let id = match request {
            SearchRequest::Start(r) => Some(r.search_id),
            SearchRequest::Page { .. } => s.last,
            SearchRequest::Cancel { .. } => None,
        };
        let token = CancellationToken::new();
        let serial = s.serial;
        s.active = Some((serial, id, token.clone()));
        Ok((
            Ticket {
                lane: self.clone(),
                serial,
            },
            token,
        ))
    }
    fn cancel_active(&self, id: KnowledgeId) -> bool {
        let s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, Some(active), token)) = &s.active {
            if *active == id {
                token.cancel();
                return true;
            }
        }
        false
    }
    pub async fn search(
        &self,
        session: &AsyncMutex<GuiSession>,
        request: SearchRequest,
    ) -> Result<SearchReply, String> {
        if let SearchRequest::Cancel { search_id } = request {
            if self.cancel_active(search_id) {
                return Ok(SearchReply::Cancelled { search_id });
            }
        }
        // Includes waiting for connection/another non-search GUI operation, but only
        // this single admitted caller can wait here. No task per keystroke queue.
        let (_ticket, token) = self.begin(&request)?;
        let mut session = acquire(session, &token).await?;
        let result = session.search_cancellable(request, token).await;
        if let Ok(SearchReply::Page(page)) = &result {
            self.0.lock().unwrap_or_else(|e| e.into_inner()).last = Some(page.search_id);
        }
        result
    }
}
async fn acquire<'a, T>(
    session: &'a AsyncMutex<T>,
    token: &CancellationToken,
) -> Result<tokio::sync::MutexGuard<'a, T>, String> {
    tokio::select! {biased;
        _=token.cancelled()=>Err("SEARCH_CANCELLED".into()),
        guard=session.lock()=>Ok(guard),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use sm_protocol::search::SearchStart;
    #[test]
    fn queued_native_admission_cancels_before_mutex_and_next_lock_works() {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async{
            let mutex=AsyncMutex::new(());let held=mutex.lock().await;let token=CancellationToken::new();
            let future=acquire(&mutex,&token);tokio::pin!(future);
            tokio::select!{_=&mut future=>panic!("lock unexpectedly acquired"),_=tokio::time::sleep(std::time::Duration::from_millis(1))=>{}}
            token.cancel();assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(1),&mut future).await.unwrap().err().unwrap(),"SEARCH_CANCELLED");
            drop(held);assert!(acquire(&mutex,&CancellationToken::new()).await.is_ok());
        });
    }
    #[test]
    fn active_cancel_is_scoped_bounded_and_ticket_releases() {
        let lane = SearchLane::default();
        let id = KnowledgeId(uuid::Uuid::new_v4());
        let request = SearchRequest::Start(SearchStart {
            search_id: id,
            text: "literal".into(),
            filters: Default::default(),
            limit: 30,
            candidate_depth: 100,
        });
        let (ticket, token) = lane.begin(&request).unwrap();
        assert_eq!(lane.begin(&request).err().unwrap(), "SEARCH_BUSY");
        assert!(!lane.cancel_active(KnowledgeId(uuid::Uuid::new_v4())));
        assert!(!token.is_cancelled());
        assert!(lane.cancel_active(id));
        assert!(token.is_cancelled());
        drop(ticket);
        let (_, next) = lane.begin(&request).unwrap();
        assert!(!next.is_cancelled());
    }
}
