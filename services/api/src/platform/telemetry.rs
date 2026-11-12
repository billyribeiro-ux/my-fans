use super::*;
/// CloudWatch Embedded Metric Format, containing aggregate counts only.
/// The bounded deployment dimension never includes account, URL, or provider IDs.
pub async fn run(s: PlatformState, mut stop: tokio::sync::watch::Receiver<bool>) {
    let Some(deployment) = std::env::var("APP_DEPLOYMENT").ok().filter(|v| {
        !v.is_empty() && v.len() <= 50 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    }) else {
        return;
    };
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    loop {
        tokio::select! {biased; _=stop.changed()=>break,_=interval.tick()=>{}}
        if let Err(e) = snapshot(&s, &deployment).await {
            tracing::error!(status = e.0.as_u16(), "operational_metrics_failed");
        }
    }
}
async fn snapshot(s: &PlatformState, deployment: &str) -> Result<()> {
    let (failed,lag,finance,mail):(i64,f64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM jobs WHERE status='failed'),(SELECT COALESCE(EXTRACT(EPOCH FROM now()-min(run_at)),0)::float8 FROM jobs WHERE status='pending' AND run_at<=now()),(SELECT count(*) FROM provider_finance_issues WHERE resolved_at IS NULL),(SELECT count(*) FROM email_deliveries WHERE transport='smtp' AND status='pending')").fetch_one(&s.db).await?;
    let scanner = if s.scanner.enabled() && s.scanner.probe().await.is_ok() {
        0
    } else {
        1
    };
    let now = chrono::Utc::now().timestamp_millis();
    println!(
        "{}",
        json!({"_aws":{"Timestamp":now,"CloudWatchMetrics":[{"Namespace":"MyFans","Dimensions":[["Deployment"]],"Metrics":[{"Name":"FailedJobs","Unit":"Count"},{"Name":"QueueLagSeconds","Unit":"Seconds"},{"Name":"OpenFinanceIssues","Unit":"Count"},{"Name":"SMTPBacklog","Unit":"Count"},{"Name":"ScannerUnavailable","Unit":"Count"},{"Name":"WorkerAlive","Unit":"Count"}]}]},"Deployment":deployment,"FailedJobs":failed,"QueueLagSeconds":lag.max(0.0),"OpenFinanceIssues":finance,"SMTPBacklog":mail,"ScannerUnavailable":scanner,"WorkerAlive":1})
    );
    Ok(())
}
