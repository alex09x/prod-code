CREATE TABLE orders (
    order_id   TEXT PRIMARY KEY,
    symbol     TEXT NOT NULL,
    price      DOUBLE PRECISION NOT NULL
);

CREATE INDEX orders_by_symbol ON orders (symbol, order_id);
