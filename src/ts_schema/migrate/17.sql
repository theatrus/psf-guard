/*
This is the migration script for TS 4 -> 5.  Also see the associated repair and update code.
*/

ALTER TABLE acquiredimage ADD COLUMN exposureId INTEGER DEFAULT 0;
ALTER TABLE acquiredimage RENAME COLUMN accepted TO gradingStatus;

ALTER TABLE imagedata ADD COLUMN width INTEGER DEFAULT 0;
ALTER TABLE imagedata ADD COLUMN height INTEGER DEFAULT 0;

ALTER TABLE profilepreference ADD COLUMN delayGrading REAL DEFAULT 80;
ALTER TABLE profilepreference ADD COLUMN autoAcceptLevelHFR REAL DEFAULT 0;
ALTER TABLE profilepreference ADD COLUMN autoAcceptLevelFWHM REAL DEFAULT 0;
ALTER TABLE profilepreference ADD COLUMN autoAcceptLevelEccentricity REAL DEFAULT 0;
ALTER TABLE profilepreference ADD COLUMN enableSimulatedRun INTEGER DEFAULT 0;
ALTER TABLE profilepreference ADD COLUMN skipSimulatedWaits INTEGER  DEFAULT 1;
ALTER TABLE profilepreference ADD COLUMN skipSimulatedUpdates INTEGER DEFAULT 0;

ALTER TABLE project ADD COLUMN maximumAltitude REAL DEFAULT 0;
ALTER TABLE project ADD COLUMN smartexposureorder INTEGER DEFAULT 0;

ALTER TABLE target RENAME COLUMN overrideExposureOrder TO unusedOEO;

CREATE TABLE IF NOT EXISTS "overrideexposureorderitem" (
   "Id"				INTEGER NOT NULL,
   "targetid"		INTEGER NOT NULL,
   "order"			INTEGER NOT NULL,
   "action"			INTEGER NOT NULL,
   "referenceIdx"	INTEGER,
   PRIMARY KEY("Id")
);

CREATE TABLE IF NOT EXISTS "filtercadenceitem" (
   "Id"				INTEGER NOT NULL,
   "targetid"		INTEGER NOT NULL,
   "order"			INTEGER NOT NULL,
   "next"			INTEGER,
   "action"			INTEGER NOT NULL,
   "referenceIdx"	INTEGER,
   PRIMARY KEY("Id")
);

PRAGMA user_version = 17;
