#include "expression.h"

int evaluate(const Expression& expression)
{
    return expression.evaluate();
}

int check_tls(int expected)
{
    return tls_zero == expected && tls_initialized == expected + 7;
}
