mod utils;

use anyhow::Result;
use bytes::Bytes;
use reqwest::StatusCode;

use depot_api::BeginBuildRequest;
use depot_client::DepotClient;
use depot_core::project::ProjectSlug;
use depot_store::blob::BlobMetadata;
use depot_store::upstream::InMemoryUpstreamCacheClient;
use depot_test_utils::{
    EXAMPLE_PROJECT_NAME, SamplePath, TestDatabase, example_project, goodbye_path, hello_path,
    sample_upstream,
};

use utils::TestApp;

async fn publish_single_path(
    client: &DepotClient,
    project: &ProjectSlug,
    ref_name: &str,
    revision: &str,
    path: SamplePath,
    body: Bytes,
) -> Result<()> {
    let begin = client
        .begin_build(BeginBuildRequest {
            project: project.as_str().to_owned(),
            ref_name: ref_name.to_owned(),
            revision: Some(revision.to_owned()),
        })
        .await?;

    let narinfo = path.narinfo();
    let register = client
        .register_paths(&begin.build_id, vec![narinfo.clone()])
        .await?;

    assert_eq!(register.required_uploads.len(), 1);

    client
        .upload_object_bytes(&begin.build_id, &path.hash(), &narinfo.url, body)
        .await?;

    client.finalize_build(&begin.build_id).await?;
    Ok(())
}

#[tokio::test]
async fn aggregate_and_project_routes_serve_published_path() -> Result<()> {
    let app = TestApp::spawn().await?;
    let write_client = app.depot_client();
    let read_client = app.http_client();

    let project = example_project();
    let path = hello_path();
    let narinfo = path.narinfo();
    let hash = path.hash();

    write_client
        .upsert_project(&project, EXAMPLE_PROJECT_NAME, true)
        .await?;

    publish_single_path(
        &write_client,
        &project,
        "refs/heads/main",
        "deadbeef",
        path,
        Bytes::from_static(b"nar-bytes"),
    )
    .await?;

    let aggregate_narinfo = read_client
        .get(app.url(format!("{}.narinfo", hash.as_str())))
        .send()
        .await?;
    assert_eq!(aggregate_narinfo.status(), StatusCode::OK);

    let aggregate_narinfo_body = aggregate_narinfo.text().await?;
    assert!(aggregate_narinfo_body.contains(&format!("StorePath: {}", narinfo.store_path)));
    assert!(aggregate_narinfo_body.contains(&format!("URL: {}", narinfo.url)));

    let aggregate_object = read_client.get(app.url(path.url())).send().await?;
    assert_eq!(aggregate_object.status(), StatusCode::OK);
    assert_eq!(
        aggregate_object.bytes().await?,
        Bytes::from_static(b"nar-bytes")
    );

    let project_narinfo = read_client
        .get(app.url(format!("p/{}/{}.narinfo", project.as_str(), hash.as_str())))
        .send()
        .await?;
    assert_eq!(project_narinfo.status(), StatusCode::OK);

    let project_object = read_client
        .get(app.url(format!("p/{}/{}", project.as_str(), path.url())))
        .send()
        .await?;
    assert_eq!(project_object.status(), StatusCode::OK);
    assert_eq!(
        project_object.bytes().await?,
        Bytes::from_static(b"nar-bytes")
    );

    Ok(())
}

#[tokio::test]
async fn project_scoped_pin_keeps_old_path_visible_after_ref_advances() -> Result<()> {
    let app = TestApp::spawn().await?;
    let write_client = app.depot_client();
    let read_client = app.http_client();

    let project = example_project();
    let first = hello_path();
    let second = goodbye_path();

    let first_hash = first.hash();
    let second_hash = second.hash();

    write_client
        .upsert_project(&project, EXAMPLE_PROJECT_NAME, true)
        .await?;

    publish_single_path(
        &write_client,
        &project,
        "refs/heads/main",
        "rev-a",
        first,
        Bytes::from_static(b"old-bytes"),
    )
    .await?;

    write_client
        .create_pin("release", Some(&project), first.store_path())
        .await?;

    publish_single_path(
        &write_client,
        &project,
        "refs/heads/main",
        "rev-b",
        second,
        Bytes::from_static(b"new-bytes"),
    )
    .await?;

    let aggregate_old = read_client
        .get(app.url(format!("{}.narinfo", first_hash.as_str())))
        .send()
        .await?;
    assert_eq!(aggregate_old.status(), StatusCode::NOT_FOUND);

    let project_old = read_client
        .get(app.url(format!(
            "p/{}/{}.narinfo",
            project.as_str(),
            first_hash.as_str()
        )))
        .send()
        .await?;
    assert_eq!(project_old.status(), StatusCode::OK);

    let project_old_object = read_client
        .get(app.url(format!("p/{}/{}", project.as_str(), first.url())))
        .send()
        .await?;
    assert_eq!(project_old_object.status(), StatusCode::OK);
    assert_eq!(
        project_old_object.bytes().await?,
        Bytes::from_static(b"old-bytes")
    );

    let aggregate_new = read_client
        .get(app.url(format!("{}.narinfo", second_hash.as_str())))
        .send()
        .await?;
    assert_eq!(aggregate_new.status(), StatusCode::OK);

    let project_new_object = read_client
        .get(app.url(format!("p/{}/{}", project.as_str(), second.url())))
        .send()
        .await?;
    assert_eq!(project_new_object.status(), StatusCode::OK);
    assert_eq!(
        project_new_object.bytes().await?,
        Bytes::from_static(b"new-bytes")
    );

    Ok(())
}

#[tokio::test]
async fn project_route_serves_upstream_backed_path_without_local_upload() -> Result<()> {
    let fixture = TestDatabase::new().await?;
    let project = fixture.insert_example_project().await?;
    let path = hello_path();
    let hash = path.hash();

    let upstream = sample_upstream("https://cache.nixos.org");

    fixture.db.insert_upstream_cache(&upstream, true).await?;
    fixture
        .db
        .link_project_upstream(&project, upstream.id)
        .await?;

    let mut upstream_client = InMemoryUpstreamCacheClient::new();
    upstream_client.insert_object(
        upstream.id,
        path.url(),
        BlobMetadata::new("application/octet-stream", Some(12), None, None),
        Bytes::from_static(b"upstream-nar"),
    );

    let app = TestApp::spawn_with_prepared_upstream(fixture.temp_dir, fixture.db, upstream_client)
        .await?;

    let write_client = app.depot_client();
    let read_client = app.http_client();

    let begin = write_client
        .begin_build(BeginBuildRequest {
            project: project.as_str().to_owned(),
            ref_name: "refs/heads/main".to_owned(),
            revision: Some("deadbeef".to_owned()),
        })
        .await?;

    let register = write_client
        .register_paths(&begin.build_id, vec![path.narinfo()])
        .await?;

    assert_eq!(register.required_uploads.len(), 0);

    write_client.finalize_build(&begin.build_id).await?;

    let project_narinfo = read_client
        .get(app.url(format!("p/{}/{}.narinfo", project.as_str(), hash.as_str())))
        .send()
        .await?;
    assert_eq!(project_narinfo.status(), StatusCode::OK);

    let project_object = read_client
        .get(app.url(format!("p/{}/{}", project.as_str(), path.url())))
        .send()
        .await?;
    assert_eq!(project_object.status(), StatusCode::OK);
    assert_eq!(
        project_object.bytes().await?,
        Bytes::from_static(b"upstream-nar")
    );

    Ok(())
}

#[tokio::test]
async fn private_project_object_is_not_readable_from_aggregate() -> Result<()> {
    let app = TestApp::spawn().await?;
    let write_client = app.depot_client();
    let read_client = app.http_client();

    let project = ProjectSlug::parse("private_repo").unwrap();
    let path = hello_path();
    let hash = path.hash();

    write_client
        .upsert_project(&project, "Private Repo", false)
        .await?;

    publish_single_path(
        &write_client,
        &project,
        "refs/heads/main",
        "deadbeef",
        path,
        Bytes::from_static(b"private-bytes"),
    )
    .await?;

    let aggregate_narinfo = read_client
        .get(app.url(format!("{}.narinfo", hash.as_str())))
        .send()
        .await?;
    assert_eq!(aggregate_narinfo.status(), StatusCode::NOT_FOUND);

    let aggregate_object = read_client.get(app.url(path.url())).send().await?;
    assert_eq!(aggregate_object.status(), StatusCode::NOT_FOUND);

    let project_object = read_client
        .get(app.url(format!("p/{}/{}", project.as_str(), path.url())))
        .send()
        .await?;
    assert_eq!(project_object.status(), StatusCode::OK);

    Ok(())
}

#[tokio::test]
async fn project_object_is_not_readable_from_other_project() -> Result<()> {
    let app = TestApp::spawn().await?;
    let write_client = app.depot_client();
    let read_client = app.http_client();

    let project_a = ProjectSlug::parse("project_a").unwrap();
    let project_b = ProjectSlug::parse("project_b").unwrap();
    let path = hello_path();
    let hash = path.hash();

    write_client
        .upsert_project(&project_a, "Project A", true)
        .await?;
    write_client
        .upsert_project(&project_b, "Project B", true)
        .await?;

    publish_single_path(
        &write_client,
        &project_a,
        "refs/heads/main",
        "deadbeef",
        path,
        Bytes::from_static(b"project-a-bytes"),
    )
    .await?;

    let project_b_narinfo = read_client
        .get(app.url(format!(
            "p/{}/{}.narinfo",
            project_b.as_str(),
            hash.as_str()
        )))
        .send()
        .await?;
    assert_eq!(project_b_narinfo.status(), StatusCode::NOT_FOUND);

    let project_b_object = read_client
        .get(app.url(format!("p/{}/{}", project_b.as_str(), path.url())))
        .send()
        .await?;
    assert_eq!(project_b_object.status(), StatusCode::NOT_FOUND);

    Ok(())
}
