//! [sea-orm](https://www.sea-ql.org/SeaORM/docs/index/) related code.
use sea_orm::sea_query::{Index, IndexCreateStatement};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, QueryOrder, QuerySelect, Schema,
};

pub mod blobs;
pub mod block_headers;

pub(crate) const BATCH_NAMESPACE: &str = "batches";
pub(crate) const PROOF_NAMESPACE: &str = "proofs";

// DB Functions

pub(crate) async fn setup_db(db: &DatabaseConnection) -> anyhow::Result<()> {
    tracing::debug!("Setting up database");
    create_tables(db, blobs::Entity).await?;
    create_tables(db, block_headers::Entity).await?;
    let builder = db.get_database_backend();
    let index_stmt: IndexCreateStatement = Index::create()
        .name("idx-blobs-block_height")
        .table(blobs::Entity)
        .col(blobs::Column::BlockHeight)
        .if_not_exists()
        .to_owned();
    db.execute(builder.build(&index_stmt)).await?;
    if let DbBackend::Sqlite = db.get_database_backend() {
        db.execute(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Sqlite,
            "PRAGMA journal_mode = WAL".to_owned(),
        ))
        .await?;
    }
    Ok(())
}

pub(crate) async fn create_tables<E: EntityTrait>(
    db: &DatabaseConnection,
    entity: E,
) -> anyhow::Result<()> {
    let builder = db.get_database_backend();
    let schema = Schema::new(builder);
    db.execute(
        builder.build(
            &schema
                .create_table_from_entity(entity)
                .if_not_exists()
                .to_owned(),
        ),
    )
    .await?;
    Ok(())
}

pub(crate) async fn query_last_height(db: &DatabaseConnection) -> anyhow::Result<u32> {
    tracing::debug!("Loading latest height from database");

    Ok(block_headers::Entity::find()
        .order_by_desc(block_headers::Column::Height)
        .select_only()
        .column(block_headers::Column::Height)
        .into_tuple::<(i32,)>()
        .one(db)
        .await?
        .map(|i| i.0 as u32)
        .unwrap_or_default())
}
