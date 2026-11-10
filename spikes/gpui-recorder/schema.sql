CREATE TABLE utterances (
    "id" INTEGER PRIMARY KEY,
    "time" INTEGER NOT NULL,
    "speaker" TEXT NOT NULL,
    "text" TEXT NOT NULL
);
CREATE TABLE notes (
    "id" INTEGER PRIMARY KEY,
    "start" INTEGER NOT NULL,
    "end" INTEGER NOT NULL,
    "label" TEXT NOT NULL,
    "body" TEXT NOT NULL,
    CHECK("end" > "start")
);
CREATE TABLE replies (
    "id" INTEGER PRIMARY KEY,
    "utterance" INTEGER NOT NULL REFERENCES utterances(id),
    "body" TEXT NOT NULL
);
CREATE TABLE span_messages (
    "span_id" INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
    "message_id" INTEGER NOT NULL REFERENCES utterances(id) ON DELETE CASCADE,
    PRIMARY KEY(span_id, message_id)
);

CREATE TABLE transcript_audio (
    "id" TEXT PRIMARY KEY,
    "utterance_id" INTEGER NOT NULL UNIQUE REFERENCES utterances(id) ON DELETE CASCADE,
    "end" INTEGER NOT NULL,
    "source" TEXT NOT NULL
);

CREATE TABLE timeline_entries (
    "id" INTEGER PRIMARY KEY,
    "time" INTEGER NOT NULL,
    "utterance_id" INTEGER REFERENCES utterances(id) ON DELETE SET NULL,
    "body" TEXT NOT NULL,
    "breakpoint" INTEGER NOT NULL CHECK("breakpoint" IN (0, 1)),
    "label" TEXT NOT NULL DEFAULT '',
    CHECK("breakpoint" = 1 OR length(trim("body")) > 0)
);

CREATE TABLE span_breakpoints (
    "span_id" INTEGER PRIMARY KEY REFERENCES notes(id) ON DELETE CASCADE,
    "start_id" INTEGER NOT NULL REFERENCES timeline_entries(id),
    "end_id" INTEGER NOT NULL REFERENCES timeline_entries(id),
    CHECK("start_id" != "end_id")
);

CREATE TABLE activity_events (
    "id" INTEGER PRIMARY KEY,
    "event_id" TEXT NOT NULL UNIQUE,
    "time" INTEGER NOT NULL,
    "application" TEXT NOT NULL,
    "action" TEXT NOT NULL,
    "payload" TEXT NOT NULL
);
CREATE INDEX activity_events_time ON activity_events("time");

-- Durable cursor feed. Sequence is ingestion order, independent of speech time.
CREATE TABLE reader_state (
    "id" INTEGER PRIMARY KEY CHECK("id" = 1),
    "database_id" TEXT NOT NULL UNIQUE,
    "initialized" INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE reader_changes (
    "seq" INTEGER PRIMARY KEY AUTOINCREMENT,
    "entity_key" TEXT NOT NULL,
    "kind" TEXT NOT NULL,
    "time" INTEGER NOT NULL,
    "previous_kind" TEXT,
    "previous_time" INTEGER,
    "operation" TEXT NOT NULL CHECK("operation" IN ('upsert','delete')),
    "payload" TEXT NOT NULL
);
CREATE INDEX reader_changes_entity ON reader_changes("entity_key", "seq");
CREATE INDEX reader_changes_time ON reader_changes("time", "seq");
CREATE TRIGGER reader_utterances_insert AFTER INSERT ON utterances
BEGIN
    INSERT INTO reader_changes("entity_key","kind","time","previous_kind","previous_time","operation","payload") VALUES('message:' || NEW."id",'transcript',NEW."time",NULL,NULL,'upsert',json_object('id','message:' || NEW."id",'kind','transcript','time_ms',NEW."time",'speaker',NEW."speaker",'text',NEW."text",'end_ms',(SELECT "end" FROM transcript_audio WHERE utterance_id=NEW."id"),'source',COALESCE((SELECT source FROM transcript_audio WHERE utterance_id=NEW."id"),CASE WHEN NEW."speaker"='Me' THEN 'microphone' ELSE 'system audio' END),'segment_id',(SELECT id FROM transcript_audio WHERE utterance_id=NEW."id")));
END;
CREATE TRIGGER reader_utterances_update AFTER UPDATE ON utterances
WHEN (OLD."id" <> NEW."id" OR (OLD."id" IS NULL AND NEW."id" IS NOT NULL) OR (OLD."id" IS NOT NULL AND NEW."id" IS NULL)) OR (OLD."time" <> NEW."time" OR (OLD."time" IS NULL AND NEW."time" IS NOT NULL) OR (OLD."time" IS NOT NULL AND NEW."time" IS NULL)) OR (OLD."speaker" <> NEW."speaker" OR (OLD."speaker" IS NULL AND NEW."speaker" IS NOT NULL) OR (OLD."speaker" IS NOT NULL AND NEW."speaker" IS NULL)) OR (OLD."text" <> NEW."text" OR (OLD."text" IS NULL AND NEW."text" IS NOT NULL) OR (OLD."text" IS NOT NULL AND NEW."text" IS NULL))
BEGIN
    INSERT INTO reader_changes("entity_key","kind","time","previous_kind","previous_time","operation","payload") VALUES('message:' || NEW."id",'transcript',NEW."time",'transcript',OLD."time",'upsert',json_object('id','message:' || NEW."id",'kind','transcript','time_ms',NEW."time",'speaker',NEW."speaker",'text',NEW."text",'end_ms',(SELECT "end" FROM transcript_audio WHERE utterance_id=NEW."id"),'source',COALESCE((SELECT source FROM transcript_audio WHERE utterance_id=NEW."id"),CASE WHEN NEW."speaker"='Me' THEN 'microphone' ELSE 'system audio' END),'segment_id',(SELECT id FROM transcript_audio WHERE utterance_id=NEW."id")));
END;
CREATE TRIGGER reader_utterances_delete AFTER DELETE ON utterances
BEGIN
    INSERT INTO reader_changes("entity_key","kind","time","previous_kind","previous_time","operation","payload") VALUES('message:' || OLD."id",'transcript',OLD."time",NULL,NULL,'delete',json_object('id','message:' || OLD."id"));
END;
CREATE TRIGGER reader_transcript_audio_insert AFTER INSERT ON transcript_audio
BEGIN
    INSERT INTO reader_changes("entity_key","kind","time","previous_kind","previous_time","operation","payload") SELECT 'message:' || u."id",'transcript',u."time",NULL,NULL,'upsert',json_object('id','message:' || u."id",'kind','transcript','time_ms',u."time",'speaker',u."speaker",'text',u."text",'end_ms',(SELECT "end" FROM transcript_audio WHERE utterance_id=u."id"),'source',COALESCE((SELECT source FROM transcript_audio WHERE utterance_id=u."id"),CASE WHEN u."speaker"='Me' THEN 'microphone' ELSE 'system audio' END),'segment_id',(SELECT id FROM transcript_audio WHERE utterance_id=u."id")) FROM utterances u WHERE u."id"=NEW."utterance_id";
END;
CREATE TRIGGER reader_transcript_audio_update AFTER UPDATE ON transcript_audio
WHEN (OLD."id" <> NEW."id" OR (OLD."id" IS NULL AND NEW."id" IS NOT NULL) OR (OLD."id" IS NOT NULL AND NEW."id" IS NULL)) OR (OLD."utterance_id" <> NEW."utterance_id" OR (OLD."utterance_id" IS NULL AND NEW."utterance_id" IS NOT NULL) OR (OLD."utterance_id" IS NOT NULL AND NEW."utterance_id" IS NULL)) OR (OLD."end" <> NEW."end" OR (OLD."end" IS NULL AND NEW."end" IS NOT NULL) OR (OLD."end" IS NOT NULL AND NEW."end" IS NULL)) OR (OLD."source" <> NEW."source" OR (OLD."source" IS NULL AND NEW."source" IS NOT NULL) OR (OLD."source" IS NOT NULL AND NEW."source" IS NULL))
BEGIN
    INSERT INTO reader_changes("entity_key","kind","time","previous_kind","previous_time","operation","payload") SELECT 'message:' || u."id",'transcript',u."time",NULL,NULL,'upsert',json_object('id','message:' || u."id",'kind','transcript','time_ms',u."time",'speaker',u."speaker",'text',u."text",'end_ms',(SELECT "end" FROM transcript_audio WHERE utterance_id=u."id"),'source',COALESCE((SELECT source FROM transcript_audio WHERE utterance_id=u."id"),CASE WHEN u."speaker"='Me' THEN 'microphone' ELSE 'system audio' END),'segment_id',(SELECT id FROM transcript_audio WHERE utterance_id=u."id")) FROM utterances u WHERE u."id"=NEW."utterance_id";
END;
CREATE TRIGGER reader_transcript_audio_delete AFTER DELETE ON transcript_audio
BEGIN
    INSERT INTO reader_changes("entity_key","kind","time","previous_kind","previous_time","operation","payload") SELECT 'message:' || u."id",'transcript',u."time",NULL,NULL,'upsert',json_object('id','message:' || u."id",'kind','transcript','time_ms',u."time",'speaker',u."speaker",'text',u."text",'end_ms',(SELECT "end" FROM transcript_audio WHERE utterance_id=u."id"),'source',COALESCE((SELECT source FROM transcript_audio WHERE utterance_id=u."id"),CASE WHEN u."speaker"='Me' THEN 'microphone' ELSE 'system audio' END),'segment_id',(SELECT id FROM transcript_audio WHERE utterance_id=u."id")) FROM utterances u WHERE u."id"=OLD."utterance_id";
END;
CREATE TRIGGER reader_timeline_entries_insert AFTER INSERT ON timeline_entries
BEGIN
    INSERT INTO reader_changes("entity_key","kind","time","previous_kind","previous_time","operation","payload") VALUES('entry:' || NEW."id",CASE WHEN NEW."breakpoint"=1 THEN 'breakpoint' ELSE 'note' END,NEW."time",NULL,NULL,'upsert',json_object('id','entry:' || NEW."id",'kind',CASE WHEN NEW."breakpoint"=1 THEN 'breakpoint' ELSE 'note' END,'time_ms',NEW."time",'text',NEW."body",'label',NEW."label",'message_id',CASE WHEN NEW."utterance_id" IS NULL THEN NULL ELSE 'message:' || NEW."utterance_id" END));
END;
CREATE TRIGGER reader_timeline_entries_update AFTER UPDATE ON timeline_entries
WHEN (OLD."id" <> NEW."id" OR (OLD."id" IS NULL AND NEW."id" IS NOT NULL) OR (OLD."id" IS NOT NULL AND NEW."id" IS NULL)) OR (OLD."time" <> NEW."time" OR (OLD."time" IS NULL AND NEW."time" IS NOT NULL) OR (OLD."time" IS NOT NULL AND NEW."time" IS NULL)) OR (OLD."utterance_id" <> NEW."utterance_id" OR (OLD."utterance_id" IS NULL AND NEW."utterance_id" IS NOT NULL) OR (OLD."utterance_id" IS NOT NULL AND NEW."utterance_id" IS NULL)) OR (OLD."body" <> NEW."body" OR (OLD."body" IS NULL AND NEW."body" IS NOT NULL) OR (OLD."body" IS NOT NULL AND NEW."body" IS NULL)) OR (OLD."breakpoint" <> NEW."breakpoint" OR (OLD."breakpoint" IS NULL AND NEW."breakpoint" IS NOT NULL) OR (OLD."breakpoint" IS NOT NULL AND NEW."breakpoint" IS NULL)) OR (OLD."label" <> NEW."label" OR (OLD."label" IS NULL AND NEW."label" IS NOT NULL) OR (OLD."label" IS NOT NULL AND NEW."label" IS NULL))
BEGIN
    INSERT INTO reader_changes("entity_key","kind","time","previous_kind","previous_time","operation","payload") VALUES('entry:' || NEW."id",CASE WHEN NEW."breakpoint"=1 THEN 'breakpoint' ELSE 'note' END,NEW."time",CASE WHEN OLD."breakpoint"=1 THEN 'breakpoint' ELSE 'note' END,OLD."time",'upsert',json_object('id','entry:' || NEW."id",'kind',CASE WHEN NEW."breakpoint"=1 THEN 'breakpoint' ELSE 'note' END,'time_ms',NEW."time",'text',NEW."body",'label',NEW."label",'message_id',CASE WHEN NEW."utterance_id" IS NULL THEN NULL ELSE 'message:' || NEW."utterance_id" END));
END;
CREATE TRIGGER reader_timeline_entries_delete AFTER DELETE ON timeline_entries
BEGIN
    INSERT INTO reader_changes("entity_key","kind","time","previous_kind","previous_time","operation","payload") VALUES('entry:' || OLD."id",CASE WHEN OLD."breakpoint"=1 THEN 'breakpoint' ELSE 'note' END,OLD."time",NULL,NULL,'delete',json_object('id','entry:' || OLD."id"));
END;
CREATE TRIGGER reader_notes_insert AFTER INSERT ON notes
BEGIN
    INSERT INTO reader_changes("entity_key","kind","time","previous_kind","previous_time","operation","payload") VALUES('span:' || NEW."id",'span',NEW."start",NULL,NULL,'upsert',json_object('id','span:' || NEW."id",'kind','span','time_ms',NEW."start",'end_ms',NEW."end",'label',NEW."label",'text',NEW."body"));
END;
CREATE TRIGGER reader_notes_update AFTER UPDATE ON notes
WHEN (OLD."id" <> NEW."id" OR (OLD."id" IS NULL AND NEW."id" IS NOT NULL) OR (OLD."id" IS NOT NULL AND NEW."id" IS NULL)) OR (OLD."start" <> NEW."start" OR (OLD."start" IS NULL AND NEW."start" IS NOT NULL) OR (OLD."start" IS NOT NULL AND NEW."start" IS NULL)) OR (OLD."end" <> NEW."end" OR (OLD."end" IS NULL AND NEW."end" IS NOT NULL) OR (OLD."end" IS NOT NULL AND NEW."end" IS NULL)) OR (OLD."label" <> NEW."label" OR (OLD."label" IS NULL AND NEW."label" IS NOT NULL) OR (OLD."label" IS NOT NULL AND NEW."label" IS NULL)) OR (OLD."body" <> NEW."body" OR (OLD."body" IS NULL AND NEW."body" IS NOT NULL) OR (OLD."body" IS NOT NULL AND NEW."body" IS NULL))
BEGIN
    INSERT INTO reader_changes("entity_key","kind","time","previous_kind","previous_time","operation","payload") VALUES('span:' || NEW."id",'span',NEW."start",'span',OLD."start",'upsert',json_object('id','span:' || NEW."id",'kind','span','time_ms',NEW."start",'end_ms',NEW."end",'label',NEW."label",'text',NEW."body"));
END;
CREATE TRIGGER reader_notes_delete AFTER DELETE ON notes
BEGIN
    INSERT INTO reader_changes("entity_key","kind","time","previous_kind","previous_time","operation","payload") VALUES('span:' || OLD."id",'span',OLD."start",NULL,NULL,'delete',json_object('id','span:' || OLD."id"));
END;
CREATE TRIGGER reader_replies_insert AFTER INSERT ON replies
BEGIN
    INSERT INTO reader_changes("entity_key","kind","time","previous_kind","previous_time","operation","payload") VALUES('comment:' || NEW."id",'comment',COALESCE((SELECT "time" FROM utterances WHERE id=NEW."utterance"),0),NULL,NULL,'upsert',json_object('id','comment:' || NEW."id",'kind','comment','time_ms',COALESCE((SELECT "time" FROM utterances WHERE id=NEW."utterance"),0),'text',NEW."body",'message_id','message:' || NEW."utterance"));
END;
CREATE TRIGGER reader_replies_update AFTER UPDATE ON replies
WHEN (OLD."id" <> NEW."id" OR (OLD."id" IS NULL AND NEW."id" IS NOT NULL) OR (OLD."id" IS NOT NULL AND NEW."id" IS NULL)) OR (OLD."utterance" <> NEW."utterance" OR (OLD."utterance" IS NULL AND NEW."utterance" IS NOT NULL) OR (OLD."utterance" IS NOT NULL AND NEW."utterance" IS NULL)) OR (OLD."body" <> NEW."body" OR (OLD."body" IS NULL AND NEW."body" IS NOT NULL) OR (OLD."body" IS NOT NULL AND NEW."body" IS NULL))
BEGIN
    INSERT INTO reader_changes("entity_key","kind","time","previous_kind","previous_time","operation","payload") VALUES('comment:' || NEW."id",'comment',COALESCE((SELECT "time" FROM utterances WHERE id=NEW."utterance"),0),'comment',COALESCE((SELECT "time" FROM utterances WHERE id=OLD."utterance"),(SELECT "time" FROM reader_changes WHERE entity_key='comment:' || OLD."id" ORDER BY seq DESC LIMIT 1),0),'upsert',json_object('id','comment:' || NEW."id",'kind','comment','time_ms',COALESCE((SELECT "time" FROM utterances WHERE id=NEW."utterance"),0),'text',NEW."body",'message_id','message:' || NEW."utterance"));
END;
CREATE TRIGGER reader_replies_delete AFTER DELETE ON replies
BEGIN
    INSERT INTO reader_changes("entity_key","kind","time","previous_kind","previous_time","operation","payload") VALUES('comment:' || OLD."id",'comment',COALESCE((SELECT "time" FROM utterances WHERE id=OLD."utterance"),(SELECT "time" FROM reader_changes WHERE entity_key='comment:' || OLD."id" ORDER BY seq DESC LIMIT 1),0),NULL,NULL,'delete',json_object('id','comment:' || OLD."id"));
END;
CREATE TRIGGER reader_activity_events_insert AFTER INSERT ON activity_events
BEGIN
    INSERT INTO reader_changes("entity_key","kind","time","previous_kind","previous_time","operation","payload") VALUES('activity:' || NEW."event_id",'activity',NEW."time",NULL,NULL,'upsert',json_object('id','activity:' || NEW."event_id",'kind','activity','time_ms',NEW."time",'application',NEW."application",'action',NEW."action",'text',COALESCE(json_extract(NEW."payload",'$.target'), ''),'window',json_extract(NEW."payload",'$.window'),'url',json_extract(NEW."payload",'$.url')));
END;
CREATE TRIGGER reader_activity_events_update AFTER UPDATE ON activity_events
WHEN (OLD."id" <> NEW."id" OR (OLD."id" IS NULL AND NEW."id" IS NOT NULL) OR (OLD."id" IS NOT NULL AND NEW."id" IS NULL)) OR (OLD."event_id" <> NEW."event_id" OR (OLD."event_id" IS NULL AND NEW."event_id" IS NOT NULL) OR (OLD."event_id" IS NOT NULL AND NEW."event_id" IS NULL)) OR (OLD."time" <> NEW."time" OR (OLD."time" IS NULL AND NEW."time" IS NOT NULL) OR (OLD."time" IS NOT NULL AND NEW."time" IS NULL)) OR (OLD."application" <> NEW."application" OR (OLD."application" IS NULL AND NEW."application" IS NOT NULL) OR (OLD."application" IS NOT NULL AND NEW."application" IS NULL)) OR (OLD."action" <> NEW."action" OR (OLD."action" IS NULL AND NEW."action" IS NOT NULL) OR (OLD."action" IS NOT NULL AND NEW."action" IS NULL)) OR (OLD."payload" <> NEW."payload" OR (OLD."payload" IS NULL AND NEW."payload" IS NOT NULL) OR (OLD."payload" IS NOT NULL AND NEW."payload" IS NULL))
BEGIN
    INSERT INTO reader_changes("entity_key","kind","time","previous_kind","previous_time","operation","payload") VALUES('activity:' || NEW."event_id",'activity',NEW."time",'activity',OLD."time",'upsert',json_object('id','activity:' || NEW."event_id",'kind','activity','time_ms',NEW."time",'application',NEW."application",'action',NEW."action",'text',COALESCE(json_extract(NEW."payload",'$.target'), ''),'window',json_extract(NEW."payload",'$.window'),'url',json_extract(NEW."payload",'$.url')));
END;
CREATE TRIGGER reader_activity_events_delete AFTER DELETE ON activity_events
BEGIN
    INSERT INTO reader_changes("entity_key","kind","time","previous_kind","previous_time","operation","payload") VALUES('activity:' || OLD."event_id",'activity',OLD."time",NULL,NULL,'delete',json_object('id','activity:' || OLD."event_id"));
END;
