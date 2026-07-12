use crate::error::AppError;
use crate::repository::Repositories;

/// List all wikis owned by a user.
pub async fn list_wikis(
    repos: &Repositories,
    owner_id: i32,
) -> Result<Vec<serde_json::Value>, AppError> {
    let wikis = repos.wiki.find_by_owner_id(owner_id).await?;

    let items: Vec<serde_json::Value> = wikis
        .into_iter()
        .map(|w| {
            serde_json::json!({
                "id": w.id,
                "name": w.name,
                "owner_id": w.owner_id,
                "published": w.published,
                "created_at": w.created_at,
            })
        })
        .collect();

    Ok(items)
}

/// Rename a wiki.
pub async fn rename_wiki(repos: &Repositories, wiki_id: i32, name: &str) -> Result<(), AppError> {
    repos.wiki.rename(wiki_id, name).await
}

/// Publish a wiki.
pub async fn publish_wiki(repos: &Repositories, wiki_id: i32) -> Result<(), AppError> {
    repos.wiki.set_published(wiki_id, true).await
}

/// Unpublish a wiki.
pub async fn unpublish_wiki(repos: &Repositories, wiki_id: i32) -> Result<(), AppError> {
    repos.wiki.set_published(wiki_id, false).await
}

/// Delete a wiki.
pub async fn delete_wiki(repos: &Repositories, wiki_id: i32) -> Result<(), AppError> {
    repos.wiki.delete_by_id(wiki_id).await
}
