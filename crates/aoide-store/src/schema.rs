//! The database's shape, versioned.
//!
//! Migrations are registered once and never edited afterwards — an edited migration
//! runs only on stores that had not already applied it, so the schema silently
//! diverges between one that upgraded and one that installed fresh. Changes go in a
//! new migration, always. The shape and the history are the phone's, v1 to v9, so a
//! row means the same thing on both.

use rusqlite::{Connection, Transaction};

const MIGRATIONS: &[(&str, &str)] = &[
    (
        "v1",
        r#"
        -- Local cache of what Melete knows. Deliberately not synced: every device
        -- rebuilds it from its own connection to the same server.
        CREATE TABLE "track" (
            "jellyfinId" TEXT PRIMARY KEY NOT NULL,
            "contentKey" TEXT NOT NULL,
            "musicbrainzId" TEXT,
            "title" TEXT NOT NULL,
            "artist" TEXT NOT NULL,
            "album" TEXT NOT NULL,
            "albumArtist" TEXT,
            "durationMs" INTEGER,
            "year" INTEGER,
            "genres" TEXT NOT NULL DEFAULT '[]',
            "lastSeenAt" INTEGER NOT NULL
        );
        CREATE INDEX "track_on_contentKey" ON "track"("contentKey");
        CREATE INDEX "track_on_musicbrainzId" ON "track"("musicbrainzId");

        CREATE TABLE "folder" (
            "id" TEXT PRIMARY KEY NOT NULL,
            "name" TEXT NOT NULL,
            "parentId" TEXT,
            "sortIndex" TEXT NOT NULL,
            "updatedAt" INTEGER NOT NULL,
            "deleted" BOOLEAN NOT NULL DEFAULT 0,
            "originDevice" TEXT NOT NULL
        );
        CREATE INDEX "folder_on_parentId" ON "folder"("parentId");

        CREATE TABLE "playlist" (
            "id" TEXT PRIMARY KEY NOT NULL,
            "name" TEXT NOT NULL,
            "notes" TEXT,
            "folderId" TEXT,
            "isSmart" BOOLEAN NOT NULL DEFAULT 0,
            "smartRules" TEXT,
            "sortIndex" TEXT NOT NULL,
            "updatedAt" INTEGER NOT NULL,
            "deleted" BOOLEAN NOT NULL DEFAULT 0,
            "originDevice" TEXT NOT NULL
        );
        CREATE INDEX "playlist_on_folderId" ON "playlist"("folderId");

        CREATE TABLE "playlistItem" (
            "id" TEXT PRIMARY KEY NOT NULL,
            "playlistId" TEXT NOT NULL,
            "jellyfinId" TEXT NOT NULL,
            "contentKey" TEXT NOT NULL,
            "position" TEXT NOT NULL,
            "updatedAt" INTEGER NOT NULL,
            "deleted" BOOLEAN NOT NULL DEFAULT 0,
            "originDevice" TEXT NOT NULL
        );
        CREATE INDEX "idx_item_playlist" ON "playlistItem"("playlistId", "position");

        CREATE TABLE "like" (
            "id" TEXT PRIMARY KEY NOT NULL,
            "jellyfinId" TEXT NOT NULL UNIQUE,
            "contentKey" TEXT NOT NULL,
            "liked" BOOLEAN NOT NULL,
            "updatedAt" INTEGER NOT NULL,
            "deleted" BOOLEAN NOT NULL DEFAULT 0,
            "originDevice" TEXT NOT NULL
        );

        -- Append-only, and the only honest source of play counts: a server may count a
        -- play at playback start, so a four-second skip counts there as a full listen.
        CREATE TABLE "playEvent" (
            "id" TEXT PRIMARY KEY NOT NULL,
            "jellyfinId" TEXT NOT NULL,
            "contentKey" TEXT NOT NULL,
            "startedAt" INTEGER NOT NULL,
            "endedAt" INTEGER,
            "msPlayed" INTEGER NOT NULL,
            "completed" BOOLEAN NOT NULL DEFAULT 0,
            "skipped" BOOLEAN NOT NULL DEFAULT 0,
            "source" TEXT,
            "originDevice" TEXT NOT NULL
        );
        CREATE INDEX "playEvent_on_jellyfinId" ON "playEvent"("jellyfinId");
        CREATE INDEX "playEvent_on_startedAt" ON "playEvent"("startedAt");

        -- One row per device, so resuming on another device reads the most recent row
        -- belonging to somebody else.
        CREATE TABLE "queueState" (
            "deviceId" TEXT PRIMARY KEY NOT NULL,
            "deviceName" TEXT NOT NULL,
            "trackIds" TEXT NOT NULL,
            "position" INTEGER NOT NULL,
            "elapsedMs" INTEGER NOT NULL,
            "updatedAt" INTEGER NOT NULL,
            "originDevice" TEXT NOT NULL
        );

        -- The outbound log. Written in the same transaction as the row it describes.
        CREATE TABLE "op" (
            "opId" TEXT PRIMARY KEY NOT NULL,
            "entity" TEXT NOT NULL,
            "entityId" TEXT NOT NULL,
            "operation" TEXT NOT NULL,
            "payload" TEXT NOT NULL,
            "createdAt" INTEGER NOT NULL,
            "synced" BOOLEAN NOT NULL DEFAULT 0
        );
        CREATE INDEX "idx_op_pending" ON "op"("synced", "createdAt");

        CREATE TABLE "syncState" (
            "key" TEXT PRIMARY KEY NOT NULL,
            "value" TEXT NOT NULL
        );
        "#,
    ),
    (
        "v2",
        r#"
        -- Where ops go when they can never succeed. Moved out of `op` rather than
        -- flagged in it, because a dead row at the head of an ordered queue blocks
        -- everything behind it forever. The payload comes along: for an inbound op it
        -- is the only copy, since the cursor has moved past it.
        CREATE TABLE "quarantinedOp" (
            "opId" TEXT PRIMARY KEY NOT NULL,
            "entity" TEXT NOT NULL,
            "entityId" TEXT NOT NULL,
            "direction" TEXT NOT NULL,
            "reason" TEXT NOT NULL,
            "payload" TEXT NOT NULL,
            "quarantinedAt" INTEGER NOT NULL
        );
        CREATE INDEX "idx_quarantine_time" ON "quarantinedOp"("quarantinedAt");
        "#,
    ),
    (
        "v3",
        r#"
        -- Nullable, and null is meaningful: a row written before per-field merging,
        -- which falls back to its own `updatedAt` for every field.
        ALTER TABLE "playlist" ADD COLUMN "fieldUpdatedAt" TEXT;
        ALTER TABLE "folder" ADD COLUMN "fieldUpdatedAt" TEXT;
        "#,
    ),
    (
        "v4",
        r#"
        -- Which Jellyfin playlist this was imported from, so a second import refreshes
        -- the same local playlist instead of making a duplicate.
        ALTER TABLE "playlist" ADD COLUMN "sourceJellyfinId" TEXT;
        CREATE INDEX "idx_playlist_source" ON "playlist"("sourceJellyfinId");
        "#,
    ),
    (
        "v5",
        r#"
        -- What a quarantined op needs to become a real op again, for the one kind of
        -- fault that gets fixed: the server's.
        ALTER TABLE "quarantinedOp" ADD COLUMN "operation" TEXT;
        ALTER TABLE "quarantinedOp" ADD COLUMN "createdAt" INTEGER;
        "#,
    ),
    (
        "v6",
        r#"
        -- A Melete album whose cover is this playlist's — never image bytes.
        ALTER TABLE "playlist" ADD COLUMN "artworkItemId" TEXT;
        -- The album a cached track belongs to, for drawing a cover from a playlist.
        ALTER TABLE "track" ADD COLUMN "albumId" TEXT;
        "#,
    ),
    (
        "v7",
        r#"
        -- A cover held as bytes elsewhere, named by the SHA-256 of those bytes.
        ALTER TABLE "playlist" ADD COLUMN "imageHash" TEXT;
        ALTER TABLE "playlist" ADD COLUMN "imageMime" TEXT;

        -- Which blobs this device holds, and whether the server has them yet. An op
        -- naming a hash must not be pushed before the bytes are on the server.
        CREATE TABLE "imageBlob" (
            "hash" TEXT PRIMARY KEY NOT NULL,
            "mime" TEXT NOT NULL,
            "bytes" INTEGER NOT NULL,
            "createdAt" INTEGER NOT NULL,
            "uploadedAt" INTEGER
        );
        "#,
    ),
    (
        "v8",
        r#"
        -- Taste flags. Their own table rather than columns on `like`, because they
        -- merge per field and a like does not.
        CREATE TABLE "trackFlags" (
            "id" TEXT PRIMARY KEY NOT NULL,
            "jellyfinId" TEXT NOT NULL UNIQUE,
            "contentKey" TEXT NOT NULL,
            "notInterested" BOOLEAN NOT NULL DEFAULT 0,
            "dontCount" BOOLEAN NOT NULL DEFAULT 0,
            "updatedAt" INTEGER NOT NULL,
            "deleted" BOOLEAN NOT NULL DEFAULT 0,
            "originDevice" TEXT NOT NULL,
            "fieldUpdatedAt" TEXT
        );
        "#,
    ),
    (
        "v9",
        r#"
        -- What the listener was doing. Nullable and never backfilled.
        ALTER TABLE "playEvent" ADD COLUMN "activity" TEXT;
        "#,
    ),
];

/// Applies every migration this store has not seen, each in its own transaction.
pub(crate) fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"CREATE TABLE IF NOT EXISTS "schemaMigration" ("identifier" TEXT PRIMARY KEY NOT NULL)"#,
    )?;
    for (identifier, sql) in MIGRATIONS {
        let tx = conn.transaction()?;
        if !applied(&tx, identifier)? {
            tx.execute_batch(sql)?;
            tx.execute(
                r#"INSERT INTO "schemaMigration" ("identifier") VALUES (?)"#,
                [identifier],
            )?;
        }
        tx.commit()?;
    }
    Ok(())
}

fn applied(tx: &Transaction<'_>, identifier: &str) -> rusqlite::Result<bool> {
    tx.query_row(
        r#"SELECT EXISTS(SELECT 1 FROM "schemaMigration" WHERE "identifier" = ?)"#,
        [identifier],
        |row| row.get(0),
    )
}
