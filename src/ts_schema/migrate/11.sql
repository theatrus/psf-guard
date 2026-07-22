/*
*/

CREATE TABLE IF NOT EXISTS `flathistory` (
   `Id`        INTEGER NOT NULL,
   `targetId`         INTEGER,
   `lightSessionDate`   INTEGER,
   `flatsTakenDate`   INTEGER,
   `profileId`    TEXT NOT NULL,
   `flatsType`    TEXT,
   `filterName`    TEXT,
   `gain`         INTEGER,
   `offset`    INTEGER,
   `bin`       INTEGER,
   `readoutmode`  INTEGER,
   `rotation`        REAL,
   `roi`        REAL,
   PRIMARY KEY(`id`)
);

ALTER TABLE project ADD COLUMN flatsHandling INTEGER NOT NULL DEFAULT 0;
UPDATE project SET flatsHandling = 0;

PRAGMA user_version = 11;
