use super::*;

#[tokio::test(start_paused = true)]
async fn takeover_rejects_the_previous_owner_and_preserves_server_forwards_registry() {
    let sessions = Arc::new(Sessions::new(Duration::from_mins(5)));
    let first = sessions.authenticate(None).await.unwrap();
    let previous = sessions.owner(&first.resume_token).await.unwrap();
    let second = sessions.authenticate(None).await.unwrap();
    assert!(previous.ended.is_cancelled());
    assert!(sessions.owner(&first.resume_token).await.is_err());
    assert!(sessions.owner(&second.resume_token).await.is_ok());
    assert!(sessions.close(&first.resume_token).await.is_err());
    assert!(sessions.owner(&second.resume_token).await.is_ok());
}

#[tokio::test(start_paused = true)]
async fn reconnect_invalidates_the_old_disconnect_timer_and_expiry_revokes_access() {
    let grace = Duration::from_mins(5);
    let sessions = Arc::new(Sessions::new(grace));
    let info = sessions.authenticate(None).await.unwrap();
    let owner = sessions.owner(&info.resume_token).await.unwrap();
    let first_socket = sessions.connect(&owner).await;
    sessions
        .disconnected(Arc::clone(&owner), first_socket)
        .await;
    tokio::task::yield_now().await;
    let second_socket = sessions.connect(&owner).await;
    tokio::time::advance(grace).await;
    tokio::task::yield_now().await;
    assert!(!owner.ended.is_cancelled());
    assert!(sessions.owner(&info.resume_token).await.is_ok());
    sessions
        .disconnected(Arc::clone(&owner), second_socket)
        .await;
    tokio::task::yield_now().await;
    tokio::time::advance(grace).await;
    owner.ended.cancelled().await;
    assert!(sessions.owner(&info.resume_token).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn zero_grace_stops_on_disconnect_and_login_without_socket_eventually_expires() {
    let sessions = Arc::new(Sessions::new(Duration::ZERO));
    let info = sessions.authenticate(None).await.unwrap();
    let owner = sessions.owner(&info.resume_token).await.unwrap();
    let socket = sessions.connect(&owner).await;
    sessions.disconnected(Arc::clone(&owner), socket).await;
    owner.ended.cancelled().await;
    assert!(sessions.owner(&info.resume_token).await.is_err());
}
