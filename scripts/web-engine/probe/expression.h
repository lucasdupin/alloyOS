#pragma once

class Expression {
public:
    virtual ~Expression() = default;
    virtual int evaluate() const = 0;
};

int evaluate(const Expression&);
extern thread_local int tls_zero;
extern thread_local int tls_initialized;
int check_tls(int expected);
