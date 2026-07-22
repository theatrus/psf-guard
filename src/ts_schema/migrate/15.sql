/*
*/

ALTER TABLE profilepreference ADD COLUMN syncEventContainerTimeout INTEGER DEFAULT 300;

PRAGMA user_version = 15;
