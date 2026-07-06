-- 歌名别名(译名/副标题等替代显示名);NULL = 未知。
-- 旧行不做数据回填:upsert 对可空字段是「非空进步」语义,数据流经自然补上。
ALTER TABLE song_meta ADD COLUMN alias TEXT;
