use rusty_ytdl::Video;

#[tokio::main]
async fn main() {
    let video_id = "A_MjCqQoLLA"; // Hey Jude
    println!("Testing video ID: {}", video_id);

    let video = Video::new(video_id).unwrap();
    match video.get_info().await {
        Ok(info) => {
            if let Some(format) = info
                .formats
                .iter()
                .filter(|f| f.has_audio && !f.has_video)
                .max_by_key(|f| f.bitrate)
            {
                println!("SUCCESS: Got URL: {}", format.url);
            } else {
                println!("FAILURE: No audio format found");
            }
        }
        Err(e) => println!("FAILURE: {}", e),
    }
}
