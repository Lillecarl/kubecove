use super::*;

#[test]
fn a_replenished_channel_cannot_starve_another_channel() {
    let hub = Arc::new(ChannelHub::default());
    let first = hub.channel::<u8>(1);
    let second = hub.channel::<u8>(2);
    first.send(1).unwrap();
    second.send(2).unwrap();
    assert_eq!(hub.pop().unwrap()["channelId"], 1);
    first.send(3).unwrap();
    assert_eq!(hub.pop().unwrap()["channelId"], 2);
    assert_eq!(hub.pop().unwrap()["message"], 3);
}

#[test]
fn retention_reports_a_gap_and_revocation_rejects_late_output() {
    let hub = Arc::new(ChannelHub::default());
    let channel = hub.channel::<usize>(1);
    for message in 0..=RETAINED_MESSAGES {
        channel.send(message).unwrap();
    }
    let gap = hub.pop().unwrap();
    assert_eq!(gap["type"], "channel_gap");
    assert_eq!(gap["dropped"], 1);
    assert_eq!(hub.pop().unwrap()["message"], 1);
    hub.remove(1);
    assert!(channel.send(0).is_err());
    assert!(hub.pop().is_none());
    let channel = hub.channel::<u8>(2);
    hub.close();
    assert!(channel.send(0).is_err());
}
