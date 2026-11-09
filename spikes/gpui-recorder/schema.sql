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
